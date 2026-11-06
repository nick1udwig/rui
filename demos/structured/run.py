#!/usr/bin/env python3
"""Reproduce authored workload exports, recovery, wire checks, pixels and codecs."""
import argparse
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[2]

def run(*args,output=None,env=None):
    result=subprocess.run([str(a) for a in args],cwd=ROOT,check=True,capture_output=output is not None,text=True,env=env)
    if output is not None:output.write_text(result.stdout)

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--seconds',type=int,default=12);p.add_argument('--skip-render',action='store_true');p.add_argument('--skip-network',action='store_true');p.add_argument('--skip-delayed',action='store_true');args=p.parse_args();out=args.output.resolve()
    run(sys.executable,'demos/structured/workloads.py','--output',out,'--seconds',args.seconds)
    for kind in ['editor','scroll','chart','diagram']:
        d=out/kind
        for prefix,start in [('publications',0),('recovery',args.seconds*25//2)]:
            run('target/release/scene-trace',d/'scenes.jsonl',d/f'{prefix}.rui',d/f'{prefix}.json',start)
            (d/f'{prefix}.rui.gz').write_bytes(gzip.compress((d/f'{prefix}.rui').read_bytes(),mtime=0))
            run('target/release/examples/compression_probe',d/f'{prefix}.rui',d/f'{prefix}.packets','history',output=d/f'{prefix}-compression.json')
            run('node','demos/games/bench/check-compression.mjs',d/f'{prefix}.rui.gz',d/f'{prefix}.packets',output=d/f'{prefix}-wasm.json')
            if not args.skip_network:run('node','demos/structured/network.mjs',d/f'{prefix}.packets',d/f'{prefix}-network.json')
        if not args.skip_network and not args.skip_delayed:run('node','demos/structured/network.mjs',d/'publications.packets',d/'delayed-network.json',20)
        if not args.skip_render:
            run('node','demos/games/bench/render.mjs',d,env=dict(os.environ,RUI_BENCH_WIDTHS='320,640',RUI_BENCH_RUNS='1'))
            run(sys.executable,'demos/structured/codecs.py',d)
    run(sys.executable,'demos/structured/summarize.py',out)
if __name__=='__main__':main()
