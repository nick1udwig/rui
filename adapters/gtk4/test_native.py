"""Bounded private IPC/retention checks; actual native effects use native_check."""
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

import gi
gi.require_version('Gtk', '4.0')
from adapters.gtk4.native import decode_request, MAX_REQUEST, NativeDispatcher
from adapters.gtk4.state import NativeRegistry, SourceRejected
from adapters.gtk4.transaction import CaptureBudget
from adapters.sdk.producer import CoherentSource
from fixtures.gtk.whole_view import WholeView


class NativeBoundsTests(unittest.TestCase):
    def test_failed_text_event_observation_permanently_denies_incarnation(self):
        for failure in ('reservation', 'read'):
            source = CoherentSource()
            fail_read = [failure == 'read']

            def read():
                if fail_read[0]:
                    raise ValueError('injected-native-read-failure')
                return {'text': {'content': 'unchanged', 'selection': [2, 2],
                                 'editable': True, 'selectable': True, 'status': 1}}

            source.register('entry', 5, read)
            source.capture_budget = CaptureBudget(limit=0 if failure == 'reservation' else 32 << 20)
            target = source.identifier('semantic:entry')
            surface = source.identifier('surface:main')
            capture = SimpleNamespace(source_alive=True, source_mapped=True,
                                      semantic_root='entry', frames=1)
            widget = SimpleNamespace(get_mapped=lambda: True, get_sensitive=lambda: True)
            registry = NativeRegistry.__new__(NativeRegistry)
            registry.source, registry.surfaces = source, {'main': capture}
            registry.widgets = {'entry': (widget, 'main')}
            registry.observe_text('entry', 'main')  # actual GObject callback target
            self.assertTrue(source.native_context_unavailable)
            self.assertEqual(source.capture_budget.used, 0)
            # A restored final value/read cannot prove a missed away/back event.
            fail_read[0] = False
            source.capture_budget.limit = 32 << 20
            registry.observe_text('entry', 'main')
            with self.assertRaisesRegex(SourceRejected, 'observation-unavailable'):
                registry.resolve_semantic(source.identity, surface, target)
            self.assertFalse(getattr(CoherentSource(), 'native_context_unavailable', False))

    def test_failed_focus_callback_permanently_denies_whole_boundary(self):
        for failure in ('read', 'ceiling'):
            whole = WholeView.__new__(WholeView)
            whole.source = CoherentSource()
            whole.previous_focus = None
            whole.focus_generation = (1 << 64) - 1 if failure == 'ceiling' else 1

            def native_focus():
                if failure == 'read':
                    raise ValueError('injected-native-focus-read-failure')
                return ('modal', 42)

            whole.native_focus = native_focus
            self.assertIsNone(whole.observe_focus())
            self.assertTrue(whole.source.native_context_unavailable)
            whole.native_focus = lambda: None
            whole.focus_generation = 1
            self.assertIsNone(whole.observe_focus())
            with self.assertRaisesRegex(SourceRejected, 'observation-unavailable'):
                whole.oracle()

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
