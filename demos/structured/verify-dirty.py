#!/usr/bin/env python3
"""Verify retained dirty rectangle byte counts by decoding and applying each PNG."""
import io
import json
from pathlib import Path
import sys
import numpy as np
from PIL import Image
for directory in [p for p in Path(sys.argv[1]).iterdir() if (p/'codecs.json').exists()]:
    count=json.loads((directory/'capture.json').read_text())['frames']
    for result in json.loads((directory/'codecs.json').read_text())['results']:
        if result['codec']!='dirty-rectangle-png':continue
        width,height=result['width'],result['height'];frames=np.memmap(directory/f'{width}.rgba',dtype=np.uint8,mode='r',shape=(count,height,width,4));prior=np.zeros_like(frames[0]);sizes=[]
        for frame in frames:
            y,x=np.where(np.any(frame!=prior,axis=2))
            if len(x):
                area=(slice(y.min(),y.max()+1),slice(x.min(),x.max()+1));png=io.BytesIO();Image.fromarray(frame[area]).save(png,format='PNG',compress_level=3);sizes.append(len(png.getvalue())+16);prior[area]=np.asarray(Image.open(io.BytesIO(png.getvalue())))
            else:sizes.append(0)
            assert np.array_equal(frame,prior)
        assert sizes==result['frameBytes']
    print(directory.name,'exact dirty-PNG reconstruction',flush=True)
