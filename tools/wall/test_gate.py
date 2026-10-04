"""The wall gate must not turn contention, omitted rounds or timeouts green."""
import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import unittest
from unittest import mock

from test_wall import load, host

gate = load('wall_gate', 'gate')


class GateTests(unittest.TestCase):
    def test_busy_is_retryable_and_does_not_build_or_reset(self):
        with tempfile.TemporaryDirectory() as home, \
                mock.patch.object(gate.Path, 'mkdir'), \
                mock.patch.object(gate.Path, 'open', return_value=open(Path(home) / 'lock', 'w')), \
                mock.patch.object(gate.fcntl, 'flock', side_effect=BlockingIOError), \
                mock.patch.object(gate.sys, 'argv', ['gate']), \
                mock.patch.object(gate.Gate, 'run') as run, \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertEqual(gate.main(), 75)
            self.assertIn('WALL GATE INCOMPLETE: gate instance busy', output.getvalue())
            run.assert_not_called()

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
                    mock.patch.object(gate.fcntl, 'flock'), \
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

    def test_each_reserved_slot_can_block_the_gate_without_any_work(self):
        for busy in range(len(gate.INSTANCES)):
            with self.subTest(instance=gate.INSTANCES[busy]), tempfile.TemporaryDirectory() as home:
                opened = []

                def lock_file(*args, **kwargs):
                    file = open(Path(home) / f'lock-{len(opened)}', 'w')
                    opened.append(file)
                    return file

                with mock.patch.object(gate.Path, 'mkdir'), \
                        mock.patch.object(gate.Path, 'open', side_effect=lock_file), \
                        mock.patch.object(gate.fcntl, 'flock', side_effect=[None] * busy + [BlockingIOError]), \
                        mock.patch.object(gate.sys, 'argv', ['gate']), \
                        mock.patch.object(gate.Gate, 'run') as run, \
                        contextlib.redirect_stdout(io.StringIO()) as output:
                    self.assertEqual(gate.main(), 75)
                self.assertTrue(all(file.closed for file in opened))
                self.assertEqual(len(opened), busy + 1)
                run.assert_not_called()
                self.assertIn('gate instance busy', output.getvalue())

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
