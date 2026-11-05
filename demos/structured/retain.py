#!/usr/bin/env python3
"""Keep compact evidence while leaving large raw frames and videos in scratch."""
import argparse
import gzip
from pathlib import Path
import shutil
p=argparse.ArgumentParser(description=__doc__);p.add_argument('source',type=Path);p.add_argument('destination',type=Path);a=p.parse_args();a.destination.mkdir(parents=True,exist_ok=True)
for name in ['summary.json','tables.md','mobile-runner.json','mobile-runner.png','live-check.json','objects-live.png','flattened-live.png','live-mobile-layout.png']:
    if (a.source/name).exists():shutil.copyfile(a.source/name,a.destination/name)
for d in a.source.iterdir():
    if not d.is_dir() or not (d/'capture.json').exists():continue
    out=a.destination/d.name;out.mkdir(exist_ok=True)
    for path in d.iterdir():
        if path.name in ['capture.json','publications.json','recovery.json','publications-compression.json','recovery-compression.json','publications-wasm.json','recovery-wasm.json','publications-network.json','recovery-network.json','delayed-network.json','codecs.json','render.json','640-reference.png','publications.rui.gz','recovery.rui.gz']:
            if path.name=='render.json':(out/'render.json.gz').write_bytes(gzip.compress(path.read_bytes(),mtime=0))
            else:shutil.copyfile(path,out/path.name)
