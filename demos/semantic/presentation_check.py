#!/usr/bin/env python3
"""Two approved browser presentations on one owned GTK source and control lease."""
import argparse
import os
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from demos.semantic.session_check import audit
from fixtures.gtk.mirror_check import run_private

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--xvfb',required=True,type=Path);p.add_argument('--output',required=True,type=Path);p.add_argument('--application',choices=('corpus','editor'),default='corpus');args=p.parse_args()
    os.environ['RUI_SEMANTIC_OUTPUT']=str(args.output.resolve());os.environ['RUI_SEMANTIC_APPLICATION']=args.application
    print(run_private(str(args.xvfb.resolve()),['node',str(ROOT/'demos/semantic/presentation-check.mjs')],timeout=150).strip())
    for name in ('mirror','semanticOnly'):audit(args.output/(name+'.json'))
if __name__=='__main__':main()
