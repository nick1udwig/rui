"""Bounded private IPC/retention checks; actual native effects use native_check."""
import json
import ctypes
import fcntl
import mmap
import os
import struct
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import gi
gi.require_version('Gtk', '4.0')
from adapters.gtk4.native import decode_request, MAX_REQUEST, NativeDispatcher
from adapters.gtk4.state import NativeRegistry, SourceRejected
from adapters.gtk4.transaction import CaptureBudget
from adapters.sdk.producer import CoherentSource
from fixtures.gtk.whole_view import WholeView
from adapters.gtk4.native_fence import NativeFence


class NativeBoundsTests(unittest.TestCase):
    def test_inherited_native_fence_has_no_absent_or_bad_descriptor_fallback(self):
        for value in (None, 'not-a-fd', '9' * 100):
            environment = {} if value is None else {'RUI_PRIVATE_CONTROL_FD': value}
            with patch.dict(os.environ, environment, clear=True):
                with self.assertRaisesRegex(ValueError, 'inherited-native-fence'):
                    NativeFence.inherited(1)
        with self.assertRaises(OSError):
            NativeFence(-1, 1)

    def test_readonly_sealed_fence_validates_identity_and_atomic_reader_lifetime(self):
        writer = os.memfd_create('rui-fence-unit', os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
        os.ftruncate(writer, 4096)
        mapping = mmap.mmap(writer, 4096, flags=mmap.MAP_SHARED, prot=mmap.PROT_READ | mmap.PROT_WRITE)
        mapping[8:24] = struct.pack('=Q', 3) + b'RUIFENC1'
        reader = os.open('/proc/self/fd/' + str(writer), os.O_RDONLY | os.O_CLOEXEC)
        try:
            with self.assertRaisesRegex(ValueError, 'size-or-access'):
                NativeFence(writer, 3)
            with self.assertRaisesRegex(ValueError, 'seals'):
                NativeFence(reader, 3)
            fcntl.fcntl(writer, fcntl.F_ADD_SEALS, fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_FUTURE_WRITE | fcntl.F_SEAL_SEAL)
            with self.assertRaisesRegex(ValueError, 'incarnation'):
                NativeFence(reader, 2)
            fence = NativeFence(reader, 3)
            self.assertEqual(fence.current(), 0)
            ctypes.c_uint64.from_buffer(mapping).value = 7
            self.assertEqual(fence.current(), 7)
            with self.assertRaises(TypeError):
                fence.mapping[0:8] = struct.pack('=Q', 99)
            ctypes.c_uint64.from_buffer(mapping).value = 0
            self.assertEqual(fence.current(), 0)
            fence.close()
            self.assertEqual(fence.current(), 0)
            fence.close()
        finally:
            mapping.close()
            os.close(reader)
            os.close(writer)

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

    def test_local_poll_only_captures_changed_source_and_respects_consumption(self):
        dispatcher = NativeDispatcher.__new__(NativeDispatcher)
        dispatcher.corpus = SimpleNamespace(source=SimpleNamespace(source_revision=12))
        dispatcher.captured_source_revision = 12
        dispatcher.promised_artifact = None
        dispatcher.pending = None
        replies, captures = [], []
        dispatcher.reply = lambda request_id, status: replies.append((request_id, status))
        dispatcher.request_capture = lambda: captures.append(dispatcher.pending)
        dispatcher.handle({'request_id': 1, 'kind': 'capture-if-changed'})
        self.assertEqual(replies, [(1, 'unchanged')])
        self.assertEqual(captures, [])
        dispatcher.corpus.source.source_revision += 1
        dispatcher.handle({'request_id': 2, 'kind': 'capture-if-changed'})
        self.assertEqual(captures, [(2, 'capture')])
        dispatcher.promised_artifact = 'whole-0002.json'
        with self.assertRaisesRegex(ValueError, 'private-unconsumed-capture'):
            dispatcher.handle({'request_id': 3, 'kind': 'capture-if-changed'})


if __name__ == '__main__':
    unittest.main()
