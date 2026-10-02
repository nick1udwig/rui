import tempfile
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from adapters.sdk.producer import CoherentSource, encode_snapshot, path_resource


class ProducerTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
