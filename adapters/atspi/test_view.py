import copy
import unittest
from unittest.mock import Mock
from adapters.atspi.view import ReadOnlyObservedView
from adapters.atspi.test_observer import candidate


class ReadOnlyViewTests(unittest.TestCase):
    def test_age_failure_scope_reduction_and_recovery_are_distinct(self):
        now=[0];port=Mock();port.pending=None
        view=ReadOnlyObservedView(port,clock=lambda:now[0]);view.scope({42:'a'*32})
        self.assertEqual(view.availability,'unavailable')
        view.refresh();port.poll.return_value=candidate();view.poll()
        wire=view.projection.last_envelope
        self.assertEqual(view.availability,'observed');self.assertIn('Hello',view.document())
        now[0]=3;self.assertEqual(view.availability,'stale')
        port.poll.return_value={'type':'unavailable','reason':'deadline'};view.poll()
        self.assertEqual(view.availability,'unavailable');self.assertEqual(view.projection.last_envelope,wire)
        view.scope({});self.assertNotIn('Hello',view.document())
        with self.assertRaises(ValueError):view.refresh()
        view.close();self.assertIsNone(view.projection.last_envelope)

    def test_remote_names_are_text_and_private_nodes_never_have_names_or_actions(self):
        port=Mock();port.pending=None
        view=ReadOnlyObservedView(port);view.scope({42:'a'*32});view.refresh()
        value=copy.deepcopy(candidate());value['apps'][0]['frames'][0]['records'][1]['name']='<script>fetch("evil")</script>'
        port.poll.return_value=value;view.poll();self.assertNotIn('<script>',view.document());self.assertIn('&lt;script&gt;',view.document())
        value['apps'][0]['frames'][0]['records'][1].update(role=40,name=None)
        view.refresh();port.poll.return_value=value;view.poll();self.assertIn('Private content',view.document())
        self.assertTrue(all(not n['actions'] for s in view.projection.last_metadata for n in s['nodes']))
        value['apps'][0]['frames'][0]['records'][1]['name']='secret'
        view.refresh();port.poll.return_value=value
        with self.assertRaises(ValueError):view.poll()
        self.assertNotIn('secret',view.document())

    def test_a_stalled_worker_cannot_delay_an_independent_view(self):
        first,second=Mock(),Mock();first.pending=second.pending=None
        a,b=ReadOnlyObservedView(first),ReadOnlyObservedView(second)
        for view in (a,b):view.scope({42:'a'*32});view.refresh()
        first.poll.return_value=None;second.poll.return_value=candidate()
        self.assertFalse(a.poll());self.assertTrue(b.poll());self.assertEqual(b.availability,'observed')
