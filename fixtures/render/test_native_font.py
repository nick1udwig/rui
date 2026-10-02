"""Pinned source shaping tests; explicitly require the native library profile."""
import hashlib
import json
from pathlib import Path
import subprocess
import unittest

ROOT=Path(__file__).resolve().parents[2]

def provider(request):
    return subprocess.run(['/usr/bin/python3',str(ROOT/'fixtures/render/native_font.py')],input=json.dumps(request).encode(),capture_output=True,timeout=5)

class NativeFontTests(unittest.TestCase):
    def test_native_shaping_preserves_source_positions_and_exact_resource_closure(self):
        result=provider({'text':'AéΩ אב','size':16})
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        self.assertEqual(data['profile'],'source.pango1582.cairo1184.liberation-sans.v1')
        self.assertEqual(data['font_sha256'],'baccc64becc3eb7d104b7c84d99f5314a0a1f896e2b3ea6c2f22fc08d2003bee')
        self.assertEqual(len(data['glyphs']),5)
        self.assertEqual([g['position'] for g in data['glyphs']],[[0,15],[11,15],[20,15],[36,15],[46,15]])
        offers={r['digest']:r for r in data['resources']}
        self.assertEqual(set(offers),{g['resource'] for g in data['glyphs']})
        for offer in offers.values():
            body=bytes.fromhex(offer['body_hex'])
            self.assertEqual(hashlib.sha256(b'RUI/0.1/resource\0' + offer['kind'].to_bytes(4,'big') + offer['version'].to_bytes(4,'big') + offer['total_bytes'].to_bytes(8,'big') + offer['provenance'].to_bytes(4,'big') + body).hexdigest(),offer['digest'])
            self.assertEqual(len(body),offer['total_bytes'])
            self.assertEqual(body[:4],b'RVP1')
            self.assertEqual((offer['kind'],offer['version'],offer['provenance']),(2,1,2))
        self.assertEqual(len(data['source_glyph_evidence']),5)

    def test_whitespace_is_native_advance_without_fake_painted_glyphs(self):
        result=provider({'text':'   ','size':16})
        self.assertEqual(result.returncode,0,result.stderr)
        data=json.loads(result.stdout)
        self.assertGreater(data['logical_bounds'][2],0)
        self.assertEqual(data['glyphs'],[])
        self.assertEqual(data['resources'],[])

    def test_missing_native_glyphs_are_explicit_without_echoing_input(self):
        result=provider({'text':'SECRET-MISSING-😀🦀','size':16})
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(result.stdout,b'')
        self.assertNotIn(b'SECRET-MISSING',result.stderr)

    def test_request_limits_fail_before_output_and_do_not_echo_text(self):
        for request in ({'text':'PRIVATE'*400,'size':16},{'text':'PRIVATE','size':float('nan')},{'text':'PRIVATE','size':257}):
            result=provider(request)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual(result.stdout,b'')
            self.assertNotIn(b'PRIVATE',result.stderr)

if __name__=='__main__':unittest.main()
