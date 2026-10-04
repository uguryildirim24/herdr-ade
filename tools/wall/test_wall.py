"""Boundary tests for the destructive tools; live proof is ./prove."""
import importlib.machinery
import importlib.util
from pathlib import Path
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
