"""Bounded private native preedit; the Rust owner supplies the only ledger.

This value never enters CoherentSource records or rendered capture resources.
Every native stage and periodic cleanup re-read the actual source context.
"""
import time

from adapters.gtk4.state import SourceRejected

MAX_TEXT_BYTES = 16 << 10


class NativeComposition:
    def __init__(self, context_reader, fence, budget=None):
        self.read_context, self.fence = context_reader, fence
        self.memory = budget.reserve(256 << 10) if budget else None
        self.active = None
        self.cancelled = 0
        self.last_cancellation = None

    def close(self):
        self.cancel('source-disposed')
        if self.memory:
            self.memory.close()
            self.memory = None

    def cancel(self, reason):
        if self.active is not None:
            self.active = None
            self.cancelled += 1
            self.last_cancellation = reason

    def poll(self):
        active = self.active
        if active is not None:
            try:
                self.check_lifetime(active['guard'])
                if self.read_context(active['guard']) != active['context']:
                    raise SourceRejected('native-composition-context-changed')
            except Exception as error:
                # Any missed native observation or failed allocation cancels
                # before a later stage can create a fresh editing context.
                self.cancel(str(error) or 'native-composition-context-unavailable')
        return True

    def check_lifetime(self, guard):
        if self.fence.current() != guard['control_generation']:
            raise SourceRejected('retired-native-lifecycle-fence')
        if time.monotonic_ns() >= guard['deadline_ns']:
            raise SourceRejected('lease-expired-at-native-dispatch')

    @staticmethod
    def valid_text(text):
        if not isinstance(text, str) or '\0' in text or len(text.encode()) > MAX_TEXT_BYTES:
            raise SourceRejected('native-composition-text-ceiling')

    @staticmethod
    def valid_range(value, count):
        if not isinstance(value, (list, tuple)) or len(value) != 2 or any(type(n) is not int for n in value) or not 0 <= value[0] <= value[1] <= count:
            raise SourceRejected('native-composition-scalar-range')

    def stage(self, request, context):
        guard = request['guard']
        self.check_lifetime(guard)
        identity, stage = request['text_session_id'], request['stage']
        if not isinstance(identity, str) or len(identity) != 32 or identity == '0' * 32 or any(c not in '0123456789abcdef' for c in identity):
            raise SourceRejected('native-composition-identity')
        if stage == 'begin':
            self.poll()
            if self.active is not None:
                raise SourceRejected('native-composition-already-active')
            self.valid_range(request['range'], len(context['content']))
            self.active = {'id': identity, 'guard': dict(guard), 'context': context,
                           'range': tuple(request['range']), 'preedit': '', 'selection': (0, 0)}
            return None
        active = self.active
        if active is None or active['id'] != identity or active['context'] != context or any(active['guard'][key] != guard[key] for key in ('lease_id', 'control_generation', 'source_generation')):
            self.cancel('native-composition-context-changed')
            raise SourceRejected('stale-native-composition')
        # A renewal can extend the same owner's deadline without losing preedit.
        active['guard']['deadline_ns'] = guard['deadline_ns']
        if stage == 'update':
            self.valid_text(request['text'])
            self.valid_range(request['selection'], len(request['text']))
            active['preedit'], active['selection'] = request['text'], tuple(request['selection'])
            return None
        if stage == 'cancel':
            self.cancel('explicit-cancel')
            return None
        if stage != 'commit':
            raise SourceRejected('unsupported-native-composition-stage')
        self.valid_text(request['text'])
        start, end = active['range']
        result = context['content'][:start] + request['text'] + context['content'][end:]
        self.valid_text(result)
        # Consume before entering the authoritative engine. No native retry
        # ledger exists here, and a partial engine failure closes the source.
        self.active = None
        return {'start': start, 'end': end, 'text': request['text']}

    def diagnostic(self):
        active = self.active
        return {'scope': 'private-controlled-fixture-only', 'active': active is not None,
                'preedit': active['preedit'] if active else None,
                'selection': list(active['selection']) if active else None,
                'cancelled': self.cancelled, 'last_cancellation': self.last_cancellation}
