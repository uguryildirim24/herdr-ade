"""Boundary tests for the destructive tools; live proof is ./prove."""
import importlib.machinery
import importlib.util
import io
import tarfile
from types import SimpleNamespace
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock


def load(name, file):
    loader = importlib.machinery.SourceFileLoader(name, str(Path(__file__).parent / file))
    spec = importlib.util.spec_from_loader(name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


guest = load('wall_guest', 'guest.py')
host = load('wall_host', 'wall')


class WallBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.home = Path(self.temporary.name)
        self.project = self.home / 'project'
        self.project.mkdir()
        self.outside = self.home / 'production-record'
        self.outside.write_text('untouched')
        patch = mock.patch.object(guest, 'PROJECT', self.project)
        patch.start()
        self.addCleanup(patch.stop)

    def test_corruption_rejects_absolute_traversal_symlink_and_directory(self):
        self.project.joinpath('link').symlink_to(self.outside)
        self.project.joinpath('directory').mkdir()
        for value in [str(self.outside), '../production-record', 'link', 'directory']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                guest.fault('corrupt', [value])
        self.assertEqual(self.outside.read_text(), 'untouched')

    def test_corruption_changes_only_the_selected_record(self):
        record = self.project / 'lane.toml'
        record.write_text('id = "t-0001"')
        guest.fault('corrupt', ['lane.toml', 'garbage'])
        self.assertEqual(record.read_bytes(), b'{invalid wall record\n')
        self.assertEqual(self.outside.read_text(), 'untouched')
        guest.fault('corrupt', ['lane.toml', 'truncate'])
        self.assertEqual(record.stat().st_size, 0)

    def test_fill_refuses_shared_disk_before_opening_a_file(self):
        with mock.patch.object(guest, 'HOME', self.home), mock.patch.object(
                guest.subprocess, 'check_output', return_value='ext4\n'):
            with self.assertRaises(ValueError):
                guest.fault('fill', [])
        self.assertFalse((self.home / 'disk-full').exists())

    def test_kill_never_accepts_an_arbitrary_pid_or_unknown_thread(self):
        with self.assertRaises(StopIteration):
            guest.fault('kill-process', ['1'])
        self.assertEqual(self.outside.read_text(), 'untouched')

    def test_codex_setup_removes_only_the_invalid_empty_provider(self):
        value = {'providers': {'opencode-go': {'modelOverrides': {}}, 'other': {'models': []}}}
        self.assertTrue(guest.remove_empty_provider(value))
        self.assertEqual(value, {'providers': {'other': {'models': []}}})
        for row in [{'modelOverrides': {'deepseek': {'contextWindow': 388384}}},
                    {'modelOverrides': {}, 'baseUrl': 'https://example.invalid'}]:
            value = {'providers': {'opencode-go': row}}
            self.assertFalse(guest.remove_empty_provider(value))
            self.assertEqual(value['providers']['opencode-go'], row)

    def test_reinstall_streams_over_ssh_without_root_writes_through_symlinks(self):
        build = self.home / 'build'
        build.mkdir()
        binary = build / 'herdr-ade'
        binary.write_bytes(b'new sandbox executable')
        stub = self.home / 'stub'
        stub.mkdir()
        (stub / 'ha').write_text('#!/bin/sh\nexit 0\n')
        (stub / 'ha').chmod(0o755)
        for box in [False, True]:
            home = self.home / ('box' if box else 'local')
            (home / 'bin').mkdir(parents=True)
            (home / 'bin/herdr-ade').symlink_to(self.outside)
            (home / 'bin/herdr-ade.next').symlink_to(self.outside)

            def sandbox_ssh(command, target_box, **kwargs):
                self.assertEqual(target_box, box)
                self.assertEqual(kwargs['stdin'].name, str(binary))
                return subprocess.run(['bash', '-c', command], check=True,
                    env=dict(os.environ, HOME=str(home), PATH=f'{stub}:/usr/bin:/bin'),
                    **kwargs)

            with mock.patch.object(host, 'HOME', self.home if box else home), \
                    mock.patch.object(host, 'require_root'), \
                    mock.patch.object(host, 'ssh', side_effect=sandbox_ssh), \
                    mock.patch.object(host.shutil, 'copy2', side_effect=AssertionError('root write')), \
                    mock.patch.object(host.sys, 'argv', ['wall', 'fault', 'install', str(build)] +
                                      (['--box'] if box else [])):
                host.main()
            self.assertEqual(self.outside.read_text(), 'untouched')
            self.assertFalse((home / 'bin/herdr-ade').is_symlink())
            self.assertEqual((home / 'bin/herdr-ade').read_bytes(), binary.read_bytes())
            self.assertEqual(list((home / 'bin').glob('herdr-ade.*')),
                             [home / 'bin/herdr-ade.next'])

    def test_successful_open_does_not_double_start_async_coordinator(self):
        project = self.project
        (project / '.state').mkdir()
        (project / '.state/coordinator.json').write_text('{"agent_name":"coordinator", "pane_id":"w1:p1"}')
        (project / '.state/coordinator-hook.json').write_text('{"pane":"w1:p1"}')
        with mock.patch.object(guest, 'HOME', self.home), \
                mock.patch.object(guest.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
                mock.patch.object(guest.subprocess, 'check_output', return_value='request test-request'), \
                mock.patch.object(guest, 'run') as run:
            guest.open_project()
        run.assert_not_called()
        self.assertEqual((self.home / 'request').read_text(), 'test-request')

    def test_d27_gate_never_uses_the_old_assisted_open_fallback(self):
        failed = subprocess.CompletedProcess(['ha', 'open', 'wall'], 1)
        with mock.patch.object(guest.subprocess, 'run', return_value=failed), \
                mock.patch.object(guest.subprocess, 'check_output') as output, \
                mock.patch.object(guest, 'run') as run:
            with self.assertRaises(subprocess.CalledProcessError):
                guest.open_project(strict=True)
        output.assert_not_called()
        run.assert_not_called()

    def test_scripted_reviewer_uses_reviewer_skill_before_mid_review(self):
        subprocess.run(['node', str(Path(__file__).with_name('test_scripted_agent.js'))],
                       check=True)

    def test_instance_names_preserve_default_and_do_not_overlap(self):
        from instance import Instance
        default = Instance()
        self.assertEqual((default.home, default.base, default.user, default.box_user,
                          default.unit, default.port, default.box_port, default.machine),
                         (Path('/home/wall'), Path('/var/lib/herdr-wall'), 'wall', 'wallbox',
                          'herdr-wall', 22285, 22286, 'wall-box'))
        instances = [Instance(n) for n in range(13)]
        for field in ['home', 'base', 'user', 'box_user', 'unit', 'machine']:
            self.assertEqual(len({getattr(i, field) for i in instances}), 13)
        self.assertEqual(len({p for i in instances for p in [i.port, i.box_port]}), 26)
        for n in [-1, 13]:
            with self.assertRaises(ValueError):
                Instance(n)

    def test_two_instance_resets_only_stop_their_own_units_and_uids(self):
        self.addCleanup(host.select, 0)
        for n in [2, 3]:
            host.select(n)
            with mock.patch.object(host, 'run') as run, mock.patch.object(host.subprocess, 'run') as pkill:
                host.stop()
                self.assertEqual(run.call_args_list, [
                    mock.call('systemctl', 'stop', f'herdr-wall-{n}-{22285 + 2*n}'),
                    mock.call('systemctl', 'stop', f'herdr-wall-{n}-{22286 + 2*n}')])
                self.assertEqual(pkill.call_args_list, [
                    mock.call(['pkill', '-KILL', '-u', f'wall{n}'], check=False),
                    mock.call(['pkill', '-KILL', '-u', f'wallbox{n}'], check=False)])

    def test_reboot_and_disconnect_use_only_selected_box_cgroup(self):
        self.addCleanup(host.select, 0)
        host.select(2)
        args = SimpleNamespace(kind='reboot', arguments=['0'], box=False, when=None, after=0)
        with mock.patch.object(host, 'run') as run, mock.patch.object(host, 'ssh') as ssh, \
                mock.patch.object(host, 'wait_port') as wait:
            host.fault(args)
            self.assertEqual(run.call_args_list, [
                mock.call('systemctl', 'stop', 'herdr-wall-2-22290'),
                mock.call('systemctl', 'start', 'herdr-wall-2-22290')])
            wait.assert_called_once_with(22290)
            ssh.assert_called_once_with('python3 "$HOME/tools/guest.py" boot', box=True)
        args.kind = 'disconnect'
        with mock.patch.object(host.subprocess, 'check_output', return_value='/system.slice/herdr-wall-2-22290.service') as output, \
                mock.patch.object(host.os, 'kill') as kill, \
                mock.patch.object(host.Path, 'rglob', return_value=[]):
            host.fault(args)
            output.assert_called_once_with(['systemctl', 'show', '-p', 'ControlGroup', '--value',
                                            'herdr-wall-2-22290'], text=True)
            kill.assert_not_called()

    def test_foreign_thread_and_foreign_uid_are_refused(self):
        threads = self.project / '.state/threads'
        threads.mkdir(parents=True)
        threads.joinpath('t-0001.toml').write_text('id = "t-0001"\npane_id = "local-pane"\n')
        with self.assertRaises(StopIteration), mock.patch.object(guest.os, 'kill') as kill:
            guest.fault('kill-process', ['t-0002'])
        kill.assert_not_called()
        # An absolute foreign record path cannot substitute for a local ID.
        with self.assertRaises(StopIteration):
            guest.fault('kill-pane', [str(self.outside)])
        runs = self.home / 'runs'
        runs.mkdir()
        (runs / 'ready.jsonl').write_text('{"pane":"local-pane","phase":"ready","pid":1}\n')
        original_stat = guest.Path.stat

        def foreign_stat(path, *args, **kwargs):
            if path == Path('/proc/1'):
                return SimpleNamespace(st_uid=os.getuid() + 1)
            return original_stat(path, *args, **kwargs)

        with mock.patch.object(guest, 'HOME', self.home), \
                mock.patch.object(guest.Path, 'stat', autospec=True, side_effect=foreign_stat), \
                mock.patch.object(guest.os, 'kill') as kill, self.assertRaises(ValueError):
            guest.fault('kill-process', ['t-0001'])
        kill.assert_not_called()

    def test_reset_never_removes_shared_auth_and_evidence_excludes_it(self):
        auth = self.home / 'shared-auth/auth.json'
        auth.parent.mkdir()
        auth.write_text('{"test":"sandbox-token"}')
        agent = self.home / '.herdr-ade/pi/agent'
        agent.mkdir(parents=True)
        (agent / 'auth.json').symlink_to(auth)
        self.project.joinpath('record.toml').write_text('id = "t-0001"')
        stream = io.BytesIO()
        with mock.patch.object(guest, 'HOME', self.home), \
                mock.patch.object(guest, 'ROOT', self.home / '.herdr-ade'), \
                mock.patch.object(guest.sys, 'stdout', SimpleNamespace(buffer=stream)):
            guest.evidence()
        with tarfile.open(fileobj=io.BytesIO(stream.getvalue())) as archive:
            self.assertFalse(any('auth' in name for name in archive.getnames()))
        self.assertNotIn(b'sandbox-token', stream.getvalue())
        self.assertNotIn('AUTH', host.reset.__code__.co_names)
        self.assertEqual(auth.read_text(), '{"test":"sandbox-token"}')

    def test_privileged_auth_never_follows_sandbox_entries(self):
        auth = self.home / 'shared-auth'
        auth.mkdir()
        file = auth / 'auth.json'
        outside_mode = self.outside.stat().st_mode
        with mock.patch.object(host, 'AUTH', auth):
            file.symlink_to(self.outside)
            with self.assertRaises(OSError):
                with host.auth_file():
                    self.fail('followed auth symlink')
            file.unlink()
            os.link(self.outside, file)
            with self.assertRaises(ValueError):
                with host.auth_file():
                    self.fail('accepted hardlinked auth')
            file.unlink()
            os.mkfifo(file)
            with self.assertRaises(ValueError):
                with host.auth_file():
                    self.fail('accepted non-regular auth')
            file.unlink()
            file.write_text('synthetic login')
            with host.auth_file() as (_, descriptor):
                # A sandbox refresh/replacement after opening cannot retarget
                # the privileged write or metadata operations.
                file.unlink()
                file.symlink_to(self.outside)
                os.ftruncate(descriptor, 0)
                os.write(descriptor, b'{}\n')
        self.assertEqual(self.outside.read_text(), 'untouched')
        self.assertEqual(self.outside.stat().st_mode, outside_mode)

    def test_auth_setup_and_logout_preserve_then_clear_only_the_regular_file(self):
        auth = self.home / 'shared-auth'
        auth.mkdir()
        file = auth / 'auth.json'
        allowed = [user for n in range(13) for user in
                   (host.Instance(n).user, host.Instance(n).box_user)]
        group = SimpleNamespace(gr_gid=os.getgid(), gr_mem=allowed)
        with mock.patch.object(host, 'AUTH', auth), \
                mock.patch.object(host.grp, 'getgrnam', return_value=group), \
                mock.patch.object(host.os, 'fchown'), \
                mock.patch.object(host, 'require_root'), \
                mock.patch.object(host.sys, 'argv', ['wall', 'logout']):
            host.shared_auth()
            self.assertEqual(file.read_text(), '{}\n')
            self.assertEqual(auth.stat().st_mode & 0o7777, 0o2770)
            self.assertEqual(file.stat().st_mode & 0o777, 0o660)
            file.write_text('synthetic login')
            host.shared_auth()
            self.assertEqual(file.read_text(), 'synthetic login')
            host.main()
            self.assertEqual(file.read_text(), '{}\n')
            file.unlink()
            file.symlink_to(self.outside)
            with self.assertRaises(OSError):
                host.shared_auth()
            with self.assertRaises(OSError):
                host.main()
        self.assertEqual(self.outside.read_text(), 'untouched')

    def test_additional_fault_dispatches_with_instance_and_rejects_traversal(self):
        self.addCleanup(host.select, 0)
        host.select(3)
        args = SimpleNamespace(kind='example', arguments=['argument'], box=True, when=None, after=0)
        with mock.patch.object(host.Path, 'is_file', return_value=True), \
                mock.patch.object(host.Path, 'is_symlink', return_value=False), \
                mock.patch.object(host.os, 'access', return_value=True), mock.patch.object(host, 'run') as run:
            host.fault(args)
            run.assert_called_once_with(Path(host.__file__).resolve().parent / 'faults/example',
                                        '--instance', '3', '--box', 'argument')
            args.kind = '../wall'
            with self.assertRaises(SystemExit):
                host.fault(args)
        args.kind = 'does-not-exist'
        with self.assertRaises(SystemExit):
            host.fault(args)

    def test_recovery_faults_accept_default_dispatch_and_refuse_numbered_sandboxes(self):
        faults = Path(__file__).parent / 'faults'
        for name in ['crash-service', 'pause-transport']:
            for instance in ['0', 'default', '1', '8']:
                with self.subTest(fault=name, instance=instance):
                    # Help exits before fault injection, but argparse validates
                    # the preceding selector exactly as wall's dispatcher does.
                    result = subprocess.run(
                        [str(faults / name), '--instance', instance, '--help'],
                        capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0 if instance in ['0', 'default'] else 2)

    def test_every_command_accepts_numbered_instance(self):
        self.addCleanup(host.select, 0)
        auth = self.home / 'flag-auth'
        auth.mkdir()
        commands = [['install', '--build', '/new/build'], ['reset'], ['enter', 'true'],
                    ['fault', 'clock', '0'], ['evidence', str(self.home / 'evidence')],
                    ['prove', str(self.home / 'proof'), '/alternate'], ['list'], ['logout']]
        for command in commands:
            with self.subTest(command=command), mock.patch.object(host.sys, 'argv', ['wall', '--instance', '12', *command]), \
                    mock.patch.object(host, 'require_root'), mock.patch.object(host, 'install'), \
                    mock.patch.object(host, 'reset'), mock.patch.object(host, 'ssh'), \
                    mock.patch.object(host, 'fault'), mock.patch.object(host, 'run'), \
                    mock.patch.object(host, 'list_instances'), mock.patch.object(host, 'shared_auth'), \
                    mock.patch.object(host, 'AUTH', auth), mock.patch.object(host.Path, 'write_text'):
                host.main()
                self.assertEqual(host.INSTANCE.number, 12)

    def test_reset_refuses_an_existing_shared_filesystem(self):
        base = self.home / 'base'
        (base / 'tools').mkdir(parents=True)
        (base / 'tools/guest.py').touch()
        with mock.patch.object(host, 'HOME', self.home), mock.patch.object(host, 'BASE', base), \
                mock.patch.object(host, 'require_root'), mock.patch.object(host, 'stop'), \
                mock.patch.object(host.os.path, 'ismount', return_value=True), \
                mock.patch.object(host.subprocess, 'check_output', return_value='ext4\n'), \
                mock.patch.object(host, 'run') as run:
            with self.assertRaises(SystemExit):
                host.reset()
            run.assert_not_called()
        self.assertEqual(self.outside.read_text(), 'untouched')


if __name__ == '__main__':
    unittest.main()
