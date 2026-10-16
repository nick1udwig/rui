import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import patch
from tempfile import TemporaryDirectory

spec = importlib.util.spec_from_file_location('browser_launcher', Path(__file__).resolve().parents[1] / 'run.py')
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)


class ListenerConfiguration(unittest.TestCase):
    def test_managed_browser_is_an_explicit_desktop_option(self):
        args = launcher.parse_args(['--http-local', '--with-browser'], workspace='desktop')
        self.assertTrue(args.with_browser)
        self.assertFalse(launcher.parse_args(['--http-local'], workspace='desktop').with_browser)

    def test_public_origin_is_independent_of_local_port(self):
        args = launcher.parse_args(['--public-url', 'https://Foo.tailnet.ts.net:443/', '--port', '8123'])
        self.assertEqual(args.origin, 'https://foo.tailnet.ts.net')
        self.assertEqual(args.port, 8123)
        self.assertEqual(args.listen, '127.0.0.1')
        self.assertIsNone(args.cert)
        args = launcher.parse_args(['--public-url', 'https://foo.tailnet.ts.net:9443'])
        self.assertEqual(args.origin, 'https://foo.tailnet.ts.net:9443')
        self.assertEqual(args.port, 8444)

    def test_public_paths_and_bare_magicdns_names(self):
        for url in ['https://Foo.tailnet.ts.net/bart', 'foo.tailnet.ts.net/bart/', 'https://foo.tailnet.ts.net:443/bart']:
            with self.subTest(url=url):
                args = launcher.parse_args(['--public-url', url])
                self.assertEqual(args.origin, 'https://foo.tailnet.ts.net')
                self.assertEqual(args.public_path, '/bart/')
        args = launcher.parse_args(['--public-url', 'foo--bar.tailnet.ts.net/tools/bart'])
        self.assertEqual(args.public_path, '/tools/bart/')

    def test_proxy_configuration_fails_closed(self):
        invalid = ['http://foo.tailnet.ts.net', 'https://foo/a/../b', 'https://foo?x=1',
                   'https://foo//bar', 'https://foo/%2e%2e/bar', 'https://foo/%2Fbar', 'https://foo/<tag>',
                   'https://foo#', 'https://user:secret@foo', 'https://foo:0',
                   'https://foo:65536', 'https://foo:', 'https://foo\\bar', 'https://foo\n']
        options = [['--public-url', url] for url in invalid]
        options += [['--public-url', 'https://foo.tailnet.ts.net', *extra] for extra in [
            ['--listen', '0.0.0.0'], ['--listen', '::'], ['--http-local'],
            ['--cert', 'cert.pem'], ['--key', 'key.pem'], ['--host', 'other']]]
        options += [['--http-local', '--listen', '0.0.0.0']]
        for args in options:
            with self.subTest(args=args), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as failure:
                    launcher.parse_args(args)
                self.assertEqual(failure.exception.code, 2)

    def test_direct_modes_preserve_transport_and_default_origin_ports(self):
        direct = launcher.parse_args(['--host', 'host.example', '--listen', '0.0.0.0', '--port', '443', '--cert', 'cert.pem', '--key', 'key.pem'])
        self.assertEqual(direct.origin, 'https://host.example')
        local = launcher.parse_args(['--http-local', '--port', '80'])
        self.assertEqual(local.origin, 'http://localhost')


class InputValidation(unittest.TestCase):
    def test_missing_browser_inputs_fail_before_build_or_state_changes(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            for workspace, flags in [('browser', []), ('desktop', ['--with-browser'])]:
                args = launcher.parse_args(['--http-local', *flags, '--files-dir', str(root/'missing'),
                                            '--state-dir', str(root/'state')], workspace=workspace)
                with self.subTest(workspace=workspace), patch.object(launcher, 'parse_args', return_value=args), patch.object(launcher, 'run') as build:
                    with self.assertRaisesRegex(SystemExit, '--files-dir'):
                        launcher.main(workspace)
                    build.assert_not_called()
                    self.assertFalse(args.state_dir.exists())

    def test_native_only_desktop_does_not_require_browser_inputs(self):
        args = launcher.parse_args(['--http-local', '--files-dir', '/nonexistent-rui-files',
                                    '--chromium', '/nonexistent-rui-chromium'], workspace='desktop')
        launcher.validate_inputs(args, 'desktop')

    def test_browser_executable_and_tls_files_are_checked(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root/'browser'
            executable.write_text('#!/bin/sh\n')
            args = launcher.parse_args(['--files-dir', str(root), '--chromium', str(executable),
                                        '--cert', str(root/'cert'), '--key', str(root/'key')])
            with self.assertRaisesRegex(SystemExit, '--chromium'):
                launcher.validate_inputs(args, 'browser')
            executable.chmod(0o700)
            for option in ('cert', 'key'):
                with self.assertRaisesRegex(SystemExit, '--' + option):
                    launcher.validate_inputs(args, 'browser')
                (root/option).write_text('fixture')
            launcher.validate_inputs(args, 'browser')
            self.assertEqual(args.chromium, str(executable.resolve()))


if __name__ == '__main__':
    unittest.main()
