"""Exercise the boundary between read-only validation and native mutation."""
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock
from apps.desktop.native import A, DesktopReader


class DispatchBoundaryTest(unittest.TestCase):
    def setUp(self):
        self.reader = object.__new__(DesktopReader)
        self.reader.GLib = SimpleNamespace(Variant=lambda *args: args)
        self.node = dict(key=(':1.2', '/button'), fingerprint='fresh',
                         actions=['Activate'], disabled=False)
        self.reader.sample = Mock(return_value={'nodes': [self.node]})
        self.reader.call = Mock(return_value=(True,))
        self.request = dict(op='dispatch', source={}, key=[':1.2', '/button'],
                            fingerprint='fresh', action='Activate', expires=(time.time() + 5) * 1000)

    def test_failed_observation_is_rejected_without_dispatch(self):
        for error in (TimeoutError(), ValueError('children changed')):
            with self.subTest(error=error):
                self.reader.sample.side_effect = error
                self.assertEqual(self.reader.execute(self.request)['status'], 'rejected')
                self.reader.call.assert_not_called()

    def test_stale_target_is_rejected_without_dispatch(self):
        self.request['fingerprint'] = 'old'
        self.assertEqual(self.reader.execute(self.request)['status'], 'rejected')
        self.reader.call.assert_not_called()

    def test_timeout_after_attempt_is_unknown_and_never_retried(self):
        self.reader.call.side_effect = TimeoutError()
        self.assertEqual(self.reader.execute(self.request), {'status': 'unknown'})
        self.reader.call.assert_called_once()

    def test_post_dispatch_observation_failure_preserves_known_result(self):
        self.reader.sample.side_effect = [{'nodes': [self.node]}, TimeoutError()]
        self.assertEqual(self.reader.execute(self.request), {'status': 'dispatched'})
        self.reader.call.assert_called_once()


class SampleReader(DesktopReader):
    def __init__(self, modals=()):
        self.GLib = SimpleNamespace(Variant=lambda signature, values: values)
        self.nodes = {
            'root': (None, 75, ['window', 'other']),
            'window': ('root', 23, ['panel', 'outside']),
            'panel': ('window', 20, ['inside', 'scroll']),
            'inside': ('panel', 43, []),
            'scroll': ('panel', 48, []),
            'outside': ('window', 43, []),
            'other': ('root', 23, ['elsewhere']),
            'elsewhere': ('other', 43, []),
        }
        self.modals = modals

    @staticmethod
    def key(name):
        return (':1.2', '/' + name)

    def verify(self, source):
        return self.key('root')

    def batch(self, requests):
        replies = []
        for key, interface, method, args in requests:
            name = key[1][1:]
            parent, role, children = self.nodes[name]
            if method == 'GetState':
                bits = (1 << 8) | (1 << 25) | (1 << 30)
                if name in self.modals:
                    bits |= 1 << 16
                answer = [bits, 0]
            elif method == 'GetRole':
                answer = role
            elif method == 'GetInterfaces':
                answer = [A + 'Action'] if role == 43 else [A + 'Value'] if role == 48 else []
            elif method == 'GetActions':
                answer = [('click', '', '')]
            elif method == 'GetChildren':
                answer = [self.key(child) for child in children]
            elif method == 'GetAll' and args == (A + 'Value',):
                answer = dict(MinimumValue=0, MaximumValue=100, CurrentValue=50, MinimumIncrement=1)
            elif method == 'GetAll' and args == (A + 'Accessible',):
                answer = dict(Name=name, Parent=self.key(parent) if parent else None, ChildCount=len(children))
            else:
                raise AssertionError((key, interface, method, args))
            replies.append((answer,))
        return replies


class ModalSampleTest(unittest.TestCase):
    def test_modal_guards_and_fingerprints_match_existing_publications(self):
        # These complete-publication fingerprints predate the subtree traversal.
        cases = [
            ((), {'inside', 'outside', 'elsewhere', 'scroll', 'scroll#scroll-back', 'scroll#scroll-forward'},
             '91a6cde102fa5534e727bd0c35b58dbe35362df785cf3889dd65475f87c300d9'),
            (('panel',), {'inside', 'scroll', 'scroll#scroll-back', 'scroll#scroll-forward'},
             '8f90c2f79c5319fdf63cbc3a3fbd895b5b94660f15a54cd4b9481ac6013932e9'),
            (('window',), {'inside', 'outside', 'scroll', 'scroll#scroll-back', 'scroll#scroll-forward'},
             '6ddf40fb71f0a81ff1d0b79dd2663b2647f1c34ac0e1df5ac7426552eb83da41'),
            (('window', 'panel'), set(),
             '5443fbe20037d29fcb487f3988afb6eb00cbe22b29122cb2126449ddf4971c3e'),
            (('panel', 'other'), set(),
             'a2e3d34dbb94254bc1224cac05148e96db7d8e9defdbbaf0e15bc10e5129667e'),
        ]
        for modals, actionable, fingerprint in cases:
            with self.subTest(modals=modals):
                sample = SampleReader(modals).sample({})
                self.assertEqual(sample['fingerprint'], fingerprint)
                self.assertEqual({node['key'][1][1:] for node in sample['nodes'] if node['actions']}, actionable)
                self.assertTrue(all(not node['disabled'] for node in sample['nodes'] if node['actions']))


if __name__ == '__main__':
    unittest.main()
