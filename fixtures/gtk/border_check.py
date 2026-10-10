#!/usr/bin/env python3
"""Fresh native comparison plus deliberate missing-paint negative control."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[2]))
from fixtures.gtk.mirror_check import run_private
from fixtures.gtk.compare import pixels


def main():
    parser=argparse.ArgumentParser(); parser.add_argument('--xvfb',required=True)
    parser.add_argument('--output',type=Path,required=True); args=parser.parse_args()
    root=Path(__file__).resolve().parents[2]
    run_private(args.xvfb,['python3',str(root/'fixtures/gtk/border_source.py'),'--output',str(args.output)])
    report=args.output/'comparison.json'
    subprocess.run(['python3',str(root/'fixtures/gtk/compare.py'),str(args.output),'--output',str(report)],check=True)
    result=json.loads(report.read_text())
    if len(result['captures'])!=4: raise RuntimeError('four fresh submitted border frames required')
    manifest=json.loads((args.output/'border-0001.json').read_text())
    with tempfile.TemporaryDirectory(prefix='rui-border-negative-') as tmp:
        pam=Path(tmp)/'negative.pam'
        subprocess.run([str(root/'target/release/examples/render_capture'),str(args.output/'border-0001.json'),str(pam),'--scene'],check=True)
        header,body=pam.read_bytes().split(b'ENDHDR\n',1); body=bytearray(body)
        removed=0
        for i in range(0,len(body),4):
            if body[i:i+4]==bytes([255,0,0,255]): body[i:i+4]=bytes([221,221,221,255]); removed+=1
        pam.write_bytes(header+b'ENDHDR\n'+body)
        negative=pixels(manifest,args.output,pam)
        if removed<100 or negative['passed']: raise RuntimeError('missing side escaped pixel comparison')
    result['negativeMissingSide']={'removedPixels':removed,'rejected':not negative['passed']}
    result['sourceHashes']={p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in
                            ('adapters/gtk4/capture.py','adapters/sdk/src/vector_audit.rs','fixtures/gtk/border_source.py','fixtures/gtk/border_check.py')}
    report.write_text(json.dumps(result,indent=2,sort_keys=True)+'\n')


if __name__=='__main__': main()
