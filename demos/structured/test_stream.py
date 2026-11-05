import io
import struct
import tempfile
from pathlib import Path
import unittest
import stream
class Fragmented:
    def __init__(self,data):self.data=io.BytesIO(data)
    def recv(self,size):return self.data.read(min(size,3))
class Stream(unittest.TestCase):
    def test_masked_browser_frames_and_protobuf_kind(self):
        for payload in [b'\x28\x1f\x32\x02\x08\x01',b'x'*200]:
            mask=b'abcd';header=bytes([0x82,128|len(payload)]) if len(payload)<126 else b'\x82\xfe'+struct.pack('>H',len(payload))
            source=Fragmented(header+mask+bytes(x^mask[i%4] for i,x in enumerate(payload)))
            self.assertEqual(stream.receive(source),payload)
        self.assertEqual(stream.fields(b'\x28\x1f\x32\x02\x08\x01'),{5:31,6:b'\x08\x01'})
    def test_truncated_and_oversized_data_fail_closed(self):
        for value in [b'\x80',b'\x32\x02x',b'\x80'*11]:
            with self.assertRaises(ValueError):stream.fields(value)
        with tempfile.TemporaryDirectory() as tmp:
            path=Path(tmp)/'packets'
            for value in [b'\x00',struct.pack('>I',2**20+1),struct.pack('>I',2)+b'x']:
                path.write_bytes(value)
                with self.assertRaises(ValueError):list(stream.packets(path))
        with self.assertRaises(ValueError):stream.receive(Fragmented(b'\x82\xfe\xff\xff'))
if __name__=='__main__':unittest.main()
