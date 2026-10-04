"""The wall gate must not turn contention, omitted rounds or timeouts green."""
import contextlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
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
                raise subprocess.TimeoutExpired('prove', 810)

            def command(subject, args, name, env=None, timeout=None):
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
            self.assertEqual([name for _, name in calls], ['final-evidence', 'final-reset', 'ubuntu-after'])
            self.assertIn('timed out at prove', output.getvalue())
            for args, name in calls[:2]:
                self.assertEqual(args[3:5], ['--instance', '5'])

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

            def command(args, name, env=None):
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

            def command(args, name, env=None):
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
