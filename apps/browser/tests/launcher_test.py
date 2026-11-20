import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import patch, Mock
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


class PhoneConfiguration(unittest.TestCase):
    def test_phone_manages_connection_options(self):
        args = launcher.parse_args(['--phone'])
        self.assertTrue(args.automatic_port)
        self.assertFalse(launcher.parse_args(['--phone', '--port', '8123']).automatic_port)
        for flags in [['--http-local'], ['--public-url', 'https://host.example'], ['--cert', 'cert.pem'],
                      ['--key', 'key.pem'], ['--host', 'other'], ['--listen', '0.0.0.0']]:
            with self.subTest(flags=flags), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    launcher.parse_args(['--phone', *flags])

    def test_phone_discovers_address_without_overwriting_existing_services(self):
        args = launcher.parse_args(['--phone'])
        config = {'TCP': {'443': {}}, 'Foreground': {'other': {'TCP': {'8443': {}}}},
                  'AllowFunnel': {'host.tailnet.ts.net:8445': True}}
        status = {'BackendState': 'Running', 'Self': {'DNSName': 'Host.tailnet.ts.net.'}}
        with patch.object(launcher, 'tailscale_json', side_effect=[status, config]), patch.object(launcher.socket, 'socket') as socket:
            socket.return_value.__enter__.return_value.getsockname.return_value = ('127.0.0.1', 49152)
            launcher.configure_phone(args)
        self.assertEqual(args.origin, 'https://host.tailnet.ts.net:8446')
        self.assertGreater(args.port, 0)
        self.assertEqual(args.public_path, '/')
        self.assertIsNone(args.cert)

    def test_signed_out_phone_fails_before_starting_a_share(self):
        with patch.object(launcher, 'tailscale_json', return_value={'BackendState': 'NeedsLogin'}):
            with self.assertRaisesRegex(SystemExit, 'Sign in'):
                launcher.configure_phone(launcher.parse_args(['--phone']))

    def test_foreground_share_is_removed_even_when_host_startup_fails(self):
        args = launcher.parse_args(['--phone', '--port', '8123'])
        args.origin = 'https://host.tailnet.ts.net:8443'
        args.phone_port = 8443
        active = {'Foreground': {'this-session': {'Web': {'host.tailnet.ts.net:8443': {
            'Handlers': {'/': {'Proxy': 'http://127.0.0.1:8123'}}}}}}}
        process = Mock()
        process.poll.return_value = None
        with patch.object(launcher, 'tailscale_json', side_effect=[{}, active]), patch.object(launcher.subprocess, 'Popen', return_value=process) as spawn:
            with self.assertRaisesRegex(RuntimeError, 'host failed'):
                with launcher.phone_share(args):
                    raise RuntimeError('host failed')
        self.assertEqual(spawn.call_args.args[0], ['tailscale', 'serve', '--https=8443', 'http://127.0.0.1:8123'])
        process.send_signal.assert_called_once_with(launcher.signal.SIGINT)
        process.wait.assert_called_once_with(timeout=5)

    def test_new_port_conflict_does_not_change_serve_config(self):
        args = launcher.parse_args(['--phone'])
        args.phone_port = 8443
        with patch.object(launcher, 'tailscale_json', return_value={'TCP': {'8443': {}}}), patch.object(launcher.subprocess, 'Popen') as spawn:
            with self.assertRaisesRegex(SystemExit, 'now in use'):
                with launcher.phone_share(args):
                    self.fail('must not share')
            spawn.assert_not_called()


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
