import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from adapters.sdk.producer import CoherentSource, encode_snapshot, path_resource


class ProducerTests(unittest.TestCase):
    def test_binding_identity_and_cache_retire_with_object_and_private_profile_denies(self):
        source = CoherentSource()
        source.register('button', 3, lambda: {'label': 'public'}, actions=['Activate'])
        binding = source.identifier('binding:button')
        first = source.bindings(10, 20, [{'id': binding, 'semantic_id': source.identifier('semantic:button'), 'interaction_region': [1, 2, 30, 20]}], usable=True)[0]
        moved = source.bindings(10, 20, [{'id': binding, 'semantic_id': first['semantic_id'], 'interaction_region': [2, 2, 30, 20]}], usable=True)[0]
        self.assertEqual(first['id'], moved['id'])
        self.assertEqual(moved['version'], first['version'] + 1)
        hidden = source.bindings(10, 20, [moved], usable=False)[0]
        self.assertIsNone(hidden['interaction_region'])
        source.unregister('button')
        self.assertIn(binding, source.retired_object_ids)
        self.assertNotIn(('binding', binding), source._models)
        source.register('button', 3, lambda: {'sensitivity': 2, 'label': 'private'}, actions=['Activate'])
        self.assertNotEqual(source.identifier('binding:button'), binding)
        with self.assertRaisesRegex(ValueError, 'private semantics unsupported'):
            source.semantics('button', 1)

    def test_scene_reappearance_allocates_fresh_id_and_stable_semantics_survive(self):
        source = CoherentSource()
        source.register("root", 1, lambda: {"label": "root"})
        semantic = source.identifier("semantic:root")
        first = source.identifier("scene:view:conditional")
        source.reconcile_scene("view", [{"id": first}])
        source.reconcile_scene("view", [])
        second = source.identifier("scene:view:conditional")
        self.assertNotEqual(first, second)
        self.assertIn(first, source.retired_scene_ids)
        source.reconcile_scene("view", [{"id": second}])
        self.assertEqual(semantic, source.identifier("semantic:root"))

    def test_destroyed_semantic_object_is_retired_and_new_incarnation_changes_epoch(self):
        source = CoherentSource()
        source.register("row", 8, lambda: {"label": "row"})
        identifier = source.identifier("semantic:row")
        source.unregister("row")
        source.register("row", 8, lambda: {"label": "new row"})
        self.assertNotEqual(identifier, source.identifier("semantic:row"))
        restarted = CoherentSource()
        self.assertNotEqual(source.identity, restarted.identity)

    def test_exact_vector_descriptor_and_resources_release_at_handoff(self):
        source = CoherentSource()
        source.register("root", 1, lambda: {"label": "root"})
        resource = path_resource([(1, (0.0, -0.0)), (2, (1.0, 1.0))])
        self.assertEqual(resource["total_bytes"], len(resource["body"]))
        source.resources[resource["digest"]] = resource
        scene = {"id": source.identifier("scene:view:root"), "version": 1, "children": [], "transform": [1, 0, 0, 1, 0, 0], "primitive": {"type": "FillPath", "resource": resource["digest"], "color": [0, 0, 0, 1]}}
        frame = {"surface_id": source.identifier("surface:view"), "surface_key": "view", "revision": 1, "size": [10, 10], "capture_token": "synthetic-test", "gaps": [], "scene_root": scene["id"], "semantic_root": source.identifier("semantic:root"), "scene": [scene], "semantics": source.semantics("root", 1), "resources": [resource["digest"]]}
        with tempfile.TemporaryDirectory() as directory:
            label = source.write(Path(directory), frame)
            self.assertTrue((Path(directory) / (label + ".bin")).exists())
            self.assertEqual((Path(directory) / (resource["digest"] + ".rvp")).read_bytes(), resource["body"])
        self.assertEqual(source.resources, {})

    def test_typed_content_collection_and_focus_clocks_and_scalar_selection(self):
        source = CoherentSource()
        value = {'content': 'Aé🦀Z', 'selection': [1, 3], 'editable': True, 'selectable': True}
        selected = []
        source.register('root', 1, lambda: {})
        source.register('entry', 5, lambda: {'text': dict(value)}, parent='root')
        source.register('list', 7, lambda: {'collection': {'items': ['row'], 'selected_items': list(selected), 'total_count': 10, 'materialization': 2}}, parent='root')
        source.register('row', 8, lambda: {'states': {3: 2 if selected else 1}}, parent='list')
        first = {node['identity']: node for node in source.semantics('root', 1)}
        self.assertEqual(first['entry']['text']['selection'], [1, 3])
        value['selection'] = [3, 1]
        second = {node['identity']: node for node in source.semantics('root', 2)}
        self.assertEqual(second['entry']['version'], first['entry']['version'])
        self.assertEqual(second['list']['version'], first['list']['version'])
        self.assertEqual(second['entry']['text']['text_version'], 1)
        self.assertEqual(second['entry']['text']['version'], first['entry']['text']['version'] + 1)
        self.assertEqual(second['list']['collection_model']['version'], 1)
        value['content'] = 'AΩZ'
        value['selection'] = [1, 2]
        selected.append('row')
        third = {node['identity']: node for node in source.semantics('root', 3)}
        self.assertEqual(third['entry']['version'], first['entry']['version'])
        self.assertGreater(third['list']['version'], second['list']['version'])
        self.assertEqual(third['entry']['text']['text_version'], 2)
        self.assertEqual(third['list']['collection_model']['version'], 2)
        self.assertEqual(third['list']['collection_model']['selected_items'], [third['row']['id']])
        source.focus_changed('view', 'entry')
        generation = source.session('view', 1, [10, 10], {third['entry']['id']})['focus_generation']
        source.focus_changed('view', None)
        source.focus_changed('view', 'entry')
        self.assertEqual(source.session('view', 2, [10, 10], {third['entry']['id']})['focus_generation'], generation + 2)
        source.unregister('entry')
        self.assertIsNone(source.session('view', 3, [10, 10], set())['focus'])
        self.assertNotIn(('text', third['entry']['text_model']), source._models)
        self.assertNotIn(('text-record', third['entry']['text_model']), source._models)

    def test_visual_refresh_preserves_both_text_clocks_and_semantic_clock(self):
        source = CoherentSource()
        source.register('entry', 5, lambda: {'text': {'content': 'Aé🦀Z', 'selection': [3, 1], 'editable': True, 'selectable': True}})
        before = source.semantics('entry', 1)[0]
        for revision in range(2, 20):
            source.changed()
            self.assertEqual(source.semantics('entry', revision)[0], before)

    def test_model_cache_quota_precedes_mutation_and_destroy_releases_charge(self):
        source = CoherentSource()
        source.register('text', 5, lambda: {'text': {'content': 'private source', 'editable': True, 'selectable': True}})
        source.semantics('text', 1)
        self.assertGreater(source.model_cache_bytes, 0)
        before = source.model_cache_bytes
        with self.assertRaises(ValueError):
            source.model_clock('text', 999, 'x' * (2 << 20))
        self.assertEqual(source.model_cache_bytes, before)
        self.assertNotIn(('text', 999), source._models)
        source.unregister('text')
        self.assertEqual(source.model_cache_bytes, 0)

    def test_surface_retirement_checks_combined_quota_before_removing_identity(self):
        source = CoherentSource()
        surface = source.identifier('surface:view')
        session = source.identifier('session:view')
        source.retired_semantic_ids.add(99)
        with patch('adapters.sdk.producer.MAX_RECORDS', 2):
            with self.assertRaisesRegex(ValueError, 'retired identity ceiling'):
                source.retire_surface('view')
        self.assertEqual(source._ids['surface:view'], surface)
        self.assertEqual(source._ids['session:view'], session)
        self.assertFalse(source.retired_object_ids)


if __name__ == "__main__":
    unittest.main()
