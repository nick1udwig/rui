import hashlib
import json
import struct
from pathlib import Path
import unittest
from conformance.full_reference import FullReplica
from conformance.reference import fields, one


class IndependentBorders(unittest.TestCase):
    def test_actual_source_paths_and_clips_survive_independent_wire_decoding(self):
        base = Path(__file__).resolve().parent
        report = json.loads((base/'border-comparison.json').read_text())
        for name, digest in report['files'].items():
            self.assertEqual(hashlib.sha256((base/'border-golden'/name).read_bytes()).hexdigest(), digest)
        for revision in range(1,5):
            root = base/'border-golden'
            manifest = json.loads((root/f'border-{revision:04}.json').read_text())
            payload = one(fields((root/f'border-{revision:04}.bin').read_bytes()),6,2)
            state = FullReplica().apply('snapshot',payload)
            scenes = [record['Scene'] for record in state['records'].values() if 'Scene' in record]
            self.assertEqual(len(scenes),len(manifest['scene']))
            paths = [scene for scene in scenes if 'FillPath' in scene['primitive']]
            self.assertEqual(len(paths),4)
            for scene in paths:
                digest = bytes(scene['primitive']['FillPath']['resource']).hex()
                self.assertIn(digest,manifest['resources'])
                body = (root/(digest+'.rvp')).read_bytes()
                descriptor = struct.pack('>IIQI',1,1,len(body),1)
                self.assertEqual(hashlib.sha256(b'RUI/0.1/resource\0'+descriptor+body).hexdigest(),digest)
            self.assertEqual(sum(scene['clip'] is not None for scene in scenes),int(revision==4))
