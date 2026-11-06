#!/usr/bin/env python3
"""Compare retained objects against the same scene flattened into compact beams."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from run import run,ROOT
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--seconds',type=int,default=12);p.add_argument('--skip-render',action='store_true');p.add_argument('--reuse-render',action='store_true');args=p.parse_args();out=args.output.resolve()
run(sys.executable,'demos/structured/objects.py','--output',out,'--seconds',args.seconds)
results={}
for variant in ['objects','flattened']:
    d=out/variant
    for prefix,start in [('publications',0),('recovery',args.seconds*25//2)]:
        run('target/release/scene-trace',d/'scenes.jsonl',d/f'{prefix}.rui',d/f'{prefix}.json',start)
        (d/f'{prefix}.rui.gz').write_bytes(gzip.compress((d/f'{prefix}.rui').read_bytes(),mtime=0))
        run('target/release/examples/compression_probe',d/f'{prefix}.rui',d/f'{prefix}.packets','history',output=d/f'{prefix}-compression.json')
        run('node','demos/games/bench/check-compression.mjs',d/f'{prefix}.rui.gz',d/f'{prefix}.packets',output=d/f'{prefix}-wasm.json')
        run('node','demos/structured/network.mjs',d/f'{prefix}.packets',d/f'{prefix}-network.json')
    if not args.skip_render:
        reuse=False
        if args.reuse_render and (d/'render.json').exists():
            cached=json.loads((d/'render.json').read_text())
            expected={'publicationStream':hashlib.sha256((d/'publications.rui').read_bytes()).hexdigest(), **{name:hashlib.sha256((ROOT/name).read_bytes()).hexdigest() for name in ['client-browser/dist/renderer.js','client-browser/wasm/rui_client_wasm_bg.wasm']}}
            reuse=all(cached['hashes'].get(k)==v for k,v in expected.items()) and [r['width'] for r in cached['results']]==[320,640,1280]
            if reuse:
                reuse=all((len(r.get('rgbaFrameSha256',[]))==r['frames']) if variant=='flattened' else ((d/f"{r['width']}.rgba").exists() and (d/f"{r['width']}.rgba").stat().st_size==r['frames']*r['width']*r['height']*4) for r in cached['results'])
        if not reuse:
            run('node','demos/games/bench/render.mjs',d,env=dict(os.environ,RUI_BENCH_WIDTHS='320,640,1280',RUI_BENCH_RUNS='1',RUI_BENCH_HASH_ONLY='1' if variant=='flattened' else '0'))
    results[variant]={key:json.loads((d/name).read_text()) for key,name in [('packets','publications-compression.json'),('network','publications-network.json'),('recovery','recovery-network.json'),('wasm','publications-wasm.json'),('publications','publications.json')]}
    results[variant]['sceneSha256']=hashlib.sha256((d/'scenes.jsonl').read_bytes()).hexdigest()
    if (d/'render.json').exists():results[variant]['render']=json.loads((d/'render.json').read_text())
if not args.skip_render:
    for a,b in zip(results['objects']['render']['results'],results['flattened']['render']['results']):
        assert a['width']==b['width'] and a['frames']==b['frames']
        size=a['width']*a['height']*4
        with (out/'objects'/f"{a['width']}.rgba").open('rb') as pixels:
            hashes=[hashlib.sha256(pixels.read(size)).hexdigest() for _ in range(a['frames'])]
            assert pixels.read()==b''
        assert hashes==b['rgbaFrameSha256'], ('pixel mismatch',a['width'])
        a['rgbaFrameSha256']=hashes
    run(sys.executable,'demos/structured/codecs.py',out/'objects')
    results['video']=json.loads((out/'objects/codecs.json').read_text())
results['seconds']=args.seconds
results['pixelEquivalenceVerified']=not args.skip_render
results['sourceHashes']={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'demos/structured/objects.py',ROOT/'browser-host/src/bin/scene-trace.rs',ROOT/'transport/src/compression.rs']}
(out/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
for key in ['objects','flattened']:
    r=results[key];print(key,'packet Mbps',r['packets']['compressed_publication_packet_bytes']*8/args.seconds/1e6,'TLS Mbps',(r['network']['tlsDownBytes']+r['network']['tlsUpBytes'])*8/args.seconds/1e6)
