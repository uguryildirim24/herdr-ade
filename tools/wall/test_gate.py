"""The wall gate must not turn contention, omitted rounds or timeouts green."""
import contextlib
import fcntl
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import unittest
from unittest import mock

from test_wall import load, host

gate = load('wall_gate', 'gate')


class GateTests(unittest.TestCase):
    def test_busy_is_retryable_and_does_not_build_or_reset(self):
        with tempfile.TemporaryDirectory() as home, contextlib.ExitStack() as held:
            directory = Path(home)
            for instances in gate.SLOT_SETS:
                for instance in instances:
                    lock = held.enter_context((directory / f'instance-{instance}.lock').open('a'))
                    fcntl.flock(lock, fcntl.LOCK_EX)
            with mock.patch.object(gate, 'LOCK_DIR', directory), \
                    mock.patch.object(gate.sys, 'argv', ['gate']), \
                    mock.patch.object(gate.Gate, 'run') as run, \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(gate.main(), 75)
                self.assertIn('WALL GATE INCOMPLETE: gate instance busy', output.getvalue())
                run.assert_not_called()

    def test_added_slots_require_explicit_post_rollout_provisioning(self):
        with tempfile.TemporaryDirectory() as home:
            directory = Path(home)
            with mock.patch.object(gate, 'WALL_BASE', directory), \
                    mock.patch.object(gate, 'LOCK_DIR', directory), \
                    mock.patch.object(gate.sys, 'argv', ['gate', '--slot-set', '9']), \
                    mock.patch.object(gate.Gate, 'run') as run, \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(gate.main(), 1)
                self.assertIn('added slot set not provisioned', output.getvalue())
                run.assert_not_called()
                self.assertFalse(list(directory.glob('instance-*.lock')))
            for instance in gate.SLOT_SETS[1]:
                manifest = directory / f'herdr-wall-{instance}' / 'tools/guest.py'
                manifest.parent.mkdir(parents=True)
                manifest.touch()
            with mock.patch.object(gate, 'WALL_BASE', directory), \
                    mock.patch.object(gate, 'LOCK_DIR', directory), \
                    mock.patch.object(gate.sys, 'argv', ['gate', '--slot-set', '9']), \
                    mock.patch.object(gate.Gate, 'run') as run, \
                    mock.patch.object(gate.Gate, 'finish', return_value=[]), \
                    mock.patch.object(signal, 'signal'), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(gate.main(), 0)
                self.assertIn('WALL SLOTS (9, 10, 11, 12)', output.getvalue())
                run.assert_called_once()

    def test_timeout_has_bounded_capture_and_reset_before_failure_result(self):
        with tempfile.TemporaryDirectory() as home:
            evidence = Path(home) / 'evidence'
            evidence.mkdir()
            calls = []

            def run(subject):
                subject.round = 'prove'
                subject.touched.update(gate.INSTANCES)
                raise subprocess.TimeoutExpired('prove', gate.BUDGET - gate.CLEANUP)

            def command(subject, args, name, env=None, timeout=None, round_name=None):
                calls.append((args, name))
                self.assertLessEqual(subject.deadline, subject.started + gate.BUDGET)

            with mock.patch.object(gate.sys, 'argv', ['gate']), \
                    mock.patch.object(gate, 'LOCK_DIR', Path(home)), \
                    mock.patch.dict(os.environ, ADE_WALL_REVIEW='0'), \
                    mock.patch.object(gate.tempfile, 'mkdtemp', return_value=str(evidence)), \
                    mock.patch.object(gate.Gate, 'run', run), \
                    mock.patch.object(gate.Gate, 'command', command), \
                    mock.patch.object(signal, 'signal'), \
                    contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(gate.main(), 1)
            self.assertEqual({name for _, name in calls},
                             {f'final-{instance}-{verb}' for instance in gate.INSTANCES
                              for verb in ['evidence', 'reset']} | {'ubuntu-after'})
            self.assertEqual(calls[-1][1], 'ubuntu-after')
            self.assertIn('timed out at prove', output.getvalue())
            for args, name in calls[:-1]:
                self.assertEqual(args[3:5], ['--instance', name.split('-')[1]])

    def test_clock_is_the_only_allowed_missing_fault_and_no_selection_override(self):
        self.assertEqual(gate.FAULTS.split(), ['kill-pane', 'kill-process', 'ticker', 'disconnect',
                                             'reboot', 'fill', 'corrupt', 'clock', 'install'])
        with tempfile.TemporaryDirectory() as home:
            home = Path(home)
            alternate = home / 'alternate'
            alternate.mkdir()
            (alternate / 'herdr-ade').write_text('alternate')
            (home / 'debug').mkdir()
            (home / 'debug/herdr-ade').write_text('candidate')
            subject = gate.Gate(home, home, alternate)
            calls = []

            def command(args, name, env=None, round_name=None, timeout=None):
                calls.append((args, name, env))
                if name == 'metadata':
                    (home / 'metadata.log').write_text(json.dumps({'target_directory': str(home)}))

            with mock.patch.object(subject, 'command', command), \
                    mock.patch.dict(os.environ, WALL_PROVE_FAULTS='clock'):
                with self.assertRaisesRegex(gate.Incomplete, 'prove kill-pane: capture missing'):
                    subject.run()
            prove = next(env for _, name, env in calls if name == 'prove')
            self.assertNotIn('WALL_PROVE_FAULTS', prove)
            self.assertFalse(any(name == 'loop' for _, name, _ in calls))

    def test_loop_restores_candidate_after_mixed_build_fault(self):
        with tempfile.TemporaryDirectory() as home:
            home = Path(home)
            alternate = home / 'alternate'
            alternate.mkdir()
            (alternate / 'herdr-ade').write_text('alternate')
            (home / 'debug').mkdir()
            (home / 'debug/herdr-ade').write_text('candidate')
            subject = gate.Gate(home, home, alternate)
            calls = []

            def command(args, name, env=None, round_name=None, timeout=None):
                calls.append((args, name))
                if name == 'metadata':
                    (home / 'metadata.log').write_text(json.dumps({'target_directory': str(home)}))
                if name == 'prove':
                    for fault in gate.FAULTS.split():
                        capture = home / 'prove' / fault / 'capture'
                        capture.mkdir(parents=True)
                        for archive in ['local.tar', 'box.tar']:
                            (capture / archive).touch()
                        (capture.parent / 'commands.log').write_text('')
                    (home / 'prove/install/commands.log').write_text(
                        'a' * 64 + ' /home/wall-5/bin/herdr-ade\n' +
                        'b' * 64 + ' /home/wall-5/bin/herdr-ade\n')

            with mock.patch.object(subject, 'command', command):
                subject.run()
            names = [name for _, name in calls]
            restored, = [args for args, name in calls if name == 'loop-install']
            self.assertEqual(restored, [*subject.wall, 'install', '--build', home / 'debug'])
            self.assertLess(names.index('prove'), names.index('loop-install'))
            self.assertLess(names.index('loop-install'), names.index('loop-build'))
            self.assertLess(names.index('loop-build'), names.index('loop'))

    def test_each_reserved_slot_blocks_its_set_and_releases_partial_locks(self):
        for instances in gate.SLOT_SETS:
            for busy in instances:
                with self.subTest(instance=busy), tempfile.TemporaryDirectory() as home:
                    directory = Path(home)
                    with (directory / f'instance-{busy}.lock').open('a') as lock:
                        fcntl.flock(lock, fcntl.LOCK_EX)
                        with gate.slots(directory, slot_sets=(instances,)) as assigned:
                            self.assertIsNone(assigned)
                        for instance in instances:
                            if instance != busy:
                                with (directory / f'instance-{instance}.lock').open('a') as probe:
                                    fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_lane_yields_to_pending_review_then_review_runs_first(self):
        with tempfile.TemporaryDirectory() as home, contextlib.ExitStack() as held:
            directory = Path(home)
            for instances in gate.SLOT_SETS:
                held.enter_context(gate.slots(directory, slot_sets=(instances,)))
            acquired, release = threading.Event(), threading.Event()
            assignment = []

            def review():
                with gate.slots(directory, review=True) as instances:
                    assignment.append(instances)
                    acquired.set()
                    release.wait(5)

            worker = threading.Thread(target=review)
            worker.start()
            try:
                deadline = time.monotonic() + 5
                while not list(directory.glob('review-*.pending')) and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(list(directory.glob('review-*.pending')))
                with gate.slots(directory) as instances:
                    self.assertIsNone(instances)
                held.close()
                self.assertTrue(acquired.wait(5))
                self.assertIn(assignment[0], gate.SLOT_SETS)
            finally:
                release.set()
                worker.join(5)
            self.assertFalse(worker.is_alive())
            self.assertFalse(list(directory.glob('review-*.pending')))

    def test_expired_review_reservation_does_not_block_lanes(self):
        with tempfile.TemporaryDirectory() as home:
            directory = Path(home)
            # A crash releases flock but leaves the reservation directory entry.
            stale = directory / 'review-dead.pending'
            with stale.open('w') as reservation:
                fcntl.flock(reservation, fcntl.LOCK_EX)
                with gate.slots(directory) as instances:
                    self.assertIsNone(instances)
            with gate.slots(directory) as instances:
                self.assertEqual(instances, gate.SLOT_SETS[0])
            self.assertFalse(stale.exists())

    def test_two_gates_hold_disjoint_sets_concurrently(self):
        with tempfile.TemporaryDirectory() as home:
            directory = Path(home)
            with gate.slots(directory) as first:
                with gate.slots(directory) as second:
                    self.assertEqual(first, (5, 6, 7, 8))
                    self.assertEqual(second, (9, 10, 11, 12))
                    with gate.slots(directory) as third:
                        self.assertIsNone(third)
                    subject = gate.Gate(directory, directory, directory, second)
                    self.assertEqual(subject.wall[-1], '9')
                    repros = [directory / f'D{i}' / 'repro' for i in range(6)]
                    with mock.patch.object(subject, 'prove'), \
                            mock.patch.object(subject, 'regressions') as run:
                        subject.campaign(repros)
                    self.assertEqual({call.args[0] for call in run.call_args_list}, {10, 11, 12})

    def test_regression_slots_and_prove_really_overlap_without_sharing(self):
        with tempfile.TemporaryDirectory() as home:
            subject = gate.Gate(Path(home), Path(home), Path(home))
            repros = [Path(home) / name / 'repro' for name in ['D27', 'D30', 'D37', 'D38', 'D39']]
            barrier = threading.Barrier(4, timeout=5)
            batches = {}

            def regressions(instance, batch):
                batches[instance] = batch
                barrier.wait()

            with mock.patch.object(subject, 'regressions', regressions), \
                    mock.patch.object(subject, 'prove', side_effect=barrier.wait):
                subject.campaign(repros)
            self.assertEqual(batches, {6: [repros[0], repros[3]], 7: [repros[1], repros[4]], 8: [repros[2]]})
            assignment = json.loads((Path(home) / 'regression-instances.json').read_text())
            self.assertEqual(set(assignment), {p.parent.name for p in repros})
            self.assertNotIn(5, assignment.values())
            timings = [json.loads(line) for line in (Path(home) / 'round-durations.jsonl').read_text().splitlines()]
            self.assertEqual(timings[0]['name'], 'prove')
            self.assertEqual(timings[0]['outcome'], 'pass')

    def test_parallel_failure_cancels_all_controllers_before_cleanup(self):
        with tempfile.TemporaryDirectory() as home:
            subject = gate.Gate(Path(home), Path(home), Path(home))
            ready = threading.Event()
            stopped = threading.Event()
            process = mock.Mock(pid=123)

            def prove():
                with subject.mutex:
                    subject.processes.add(process)
                ready.set()
                self.assertTrue(stopped.wait(5))
                with subject.mutex:
                    subject.processes.discard(process)

            def regressions(instance, batch):
                self.assertTrue(ready.wait(5))
                raise gate.RoundFailure('regression D37', 'real defect')

            with mock.patch.object(subject, 'prove', prove), \
                    mock.patch.object(subject, 'regressions', regressions), \
                    mock.patch.object(subject, 'kill', side_effect=lambda _: stopped.set()) as kill:
                with self.assertRaisesRegex(gate.RoundFailure, 'real defect'):
                    subject.campaign([Path(home) / 'D37/repro'])
            kill.assert_called_once_with(process)
            self.assertFalse(subject.processes)
            self.assertTrue(subject.aborting)

    def test_cleanup_is_parallel_within_the_total_budget(self):
        with tempfile.TemporaryDirectory() as home:
            subject = gate.Gate(Path(home), Path(home), Path(home))
            subject.touched.update(gate.INSTANCES)
            barrier = threading.Barrier(4, timeout=5)

            def command(args, name, **kwargs):
                self.assertLess(subject.deadline, subject.started + gate.BUDGET)
                if name.endswith('-evidence'):
                    barrier.wait()

            with mock.patch.object(subject, 'command', command):
                self.assertEqual(subject.finish(), [])
            self.assertEqual(gate.BUDGET, 1500)
            self.assertEqual(gate.CLEANUP, 90)
            self.assertLess(gate.BUDGET, 1800)

    def test_only_the_clock_finding_is_allowed_not_assisted_open_or_provider_workarounds(self):
        with tempfile.TemporaryDirectory() as home:
            log = Path(home) / 'commands.log'
            log.write_text('FINDING: no injectable harness clock. No host time changed.\n')
            gate.findings(log, clock=True)
            with self.assertRaisesRegex(RuntimeError, 'no injectable'):
                gate.findings(log)
            for line in ['FINDING D27: explicit sandbox bootstrap after ha open timeout',
                         'FINDING: setup emitted an empty opencode-go provider; removed']:
                log.write_text(line)
                with self.assertRaises(RuntimeError):
                    gate.findings(log, clock=True)

    def test_new_binary_stage_reuses_existing_pinned_package_without_npm(self):
        with tempfile.TemporaryDirectory() as home:
            stages = Path(home)
            prior = stages / 'prior-build'
            manifest = prior / 'pi/node_modules/@earendil-works/pi-coding-agent/package.json'
            manifest.parent.mkdir(parents=True)
            manifest.write_text('{"version":"0.99.1"}')
            backend = manifest.parent / 'dist/core/auth-storage.js'
            backend.parent.mkdir(parents=True)
            backend.write_text('realpath: true; realpath: true;')
            (prior / 'build.json').write_text('{}')
            (prior / '.npm/_cacache').mkdir(parents=True)
            (prior / 'npm').mkdir()
            with mock.patch.object(host, 'STAGES', stages), mock.patch.object(host, 'run') as run:
                package = host.package_stage(None, stages / 'candidate.building', '/node')
                self.assertEqual((package / 'pi').resolve(), (prior / 'pi').resolve())
                self.assertEqual(host.package_stage(None, stages / 'second.building', '/node'), package)
                run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
