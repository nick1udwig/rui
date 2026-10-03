"""Bounded private IPC/retention checks; actual native effects use native_check."""
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import gi
gi.require_version('Gtk', '4.0')
from adapters.gtk4.native import decode_request, MAX_REQUEST, NativeDispatcher


class NativeBoundsTests(unittest.TestCase):
    def test_json_preflight_and_worst_case_admitted_text(self):
        body = json.dumps({'request_id': 1, 'arguments': {'text': '\x01' * (16 << 10)}}).encode()
        self.assertGreater(len(body), 64 << 10)
        self.assertLess(len(body), MAX_REQUEST)
        self.assertEqual(decode_request(body)['arguments']['text'], '\x01' * (16 << 10))
        for body in (b'x' * (MAX_REQUEST + 1), b'{"request_id":true}',
                     b'{"request_id":1,"x":' + b'[' * 9 + b'0' + b']' * 9 + b'}',
                     b'{"request_id":1,"x":[' + b','.join([b'0'] * 130) + b']}'):
            with self.assertRaises(ValueError):
                decode_request(body)

    def test_consumed_capture_ownership_preserves_shared_bodies_and_prunes_churn(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            dispatcher = NativeDispatcher.__new__(NativeDispatcher)
            dispatcher.whole = None
            dispatcher.corpus = SimpleNamespace(output=output)
            dispatcher.current_artifact = 'first.json'
            dispatcher.promised_artifact = 'second.json'
            dispatcher.artifacts = {'first.json': {'first.json', 'first.bin', 'shared.rvp', 'old.rvp'},
                                    'second.json': {'second.json', 'second.bin', 'shared.rvp', 'new.rvp'},
                                    'unclaimed.json': {'unclaimed.json', 'unclaimed.bin', 'shared.rvp'}}
            for files in dispatcher.artifacts.values():
                for name in files:
                    (output / name).write_bytes(b'actual-bounded-body')
            dispatcher.prune_artifacts()
            self.assertTrue((output / 'old.rvp').exists())
            self.assertFalse((output / 'unclaimed.json').exists())
            dispatcher.current_artifact = dispatcher.promised_artifact
            dispatcher.promised_artifact = None
            dispatcher.prune_artifacts()
            self.assertEqual({path.name for path in output.iterdir()}, {'second.json', 'second.bin', 'shared.rvp', 'new.rvp'})
            for i in range(100):
                name = f'churn-{i}.json'
                (output / name).write_bytes(b'unsolicited')
                dispatcher.artifacts[name] = {name, 'shared.rvp'}
                dispatcher.prune_artifacts()
                self.assertEqual(len(dispatcher.artifacts), 1)
                self.assertFalse((output / name).exists())


if __name__ == '__main__':
    unittest.main()
