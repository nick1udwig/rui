#!/usr/bin/env python3
"""Owned-display live semantic session plus independent full-record wire replay."""
import argparse
import json
import os
from pathlib import Path
import sys
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from fixtures.gtk.mirror_check import run_private
from conformance.semantic_client import decode, binary, value
from conformance.full_reference import FullReplica


def audit(path):
    if path.stat().st_size > 2<<20: raise ValueError('evidence ceiling')
    report=json.loads(path.read_text());replica=FullReplica();pending=None;prefixes=0;scope=None;scopes=0
    for item in report['evidence']:
        if item['direction']!='received':continue
        packet=decode(bytes.fromhex(item['hex']),set(range(1,7)))
        kind=value(packet,5);payload=binary(packet,6)
        if kind==2:
            incoming=tuple(binary(packet,t) for t in (1,2,3))
            if incoming!=scope:replica=FullReplica();pending=None;scope=incoming;scopes+=1
        if kind in (10,11):
            begin=decode(payload,set(range(1,8)))
            pending=(kind,begin,bytearray())
        elif kind==12:
            chunk=decode(payload,{1,2,3})
            assert pending and binary(chunk,1)==binary(pending[1],1)
            assert value(chunk,2)==0 # These owned small projections fit one chunk.
            pending[2].extend(binary(chunk,3))
        elif kind==13:
            assert pending
            commit=decode(payload,{1,2,3,4,5})
            assert binary(commit,1)==binary(pending[1],1)
            assert len(pending[2])==value(commit,2)
            import hashlib
            assert hashlib.sha256(pending[2]).digest()==binary(commit,5)==binary(pending[1],7)
            replica.apply('snapshot' if pending[0]==10 else 'transaction',bytes(pending[2]));pending=None;prefixes+=1
    def clocks(obj,key=''):
        keys={'revision','id','version','text_version','focus_generation','reading_order','total_count','anchor','focus','stacking_order','Integer'}
        if isinstance(obj,int) and not isinstance(obj,bool) and key in keys:return str(obj)
        if isinstance(obj,list):return [str(v) if key in ('retired','scalar_clusters') else clocks(v) for v in obj]
        if isinstance(obj,dict):return {k:clocks(v,k) for k,v in obj.items()}
        return obj
    assert clocks(replica.state())==report['state'],'independent wire/client state differs'
    assert prefixes>=3 and len(report['effects'])==3
    report['independentPythonPrefixes']=prefixes
    report['independentScopes']=scopes
    path.write_text(json.dumps(report,indent=2)+'\n')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--xvfb',required=True,type=Path)
    parser.add_argument('--application',choices=('corpus','editor'),default='corpus')
    parser.add_argument('--dom-controls',action='store_true')
    parser.add_argument('--output',required=True,type=Path)
    args=parser.parse_args()
    os.environ['RUI_SEMANTIC_OUTPUT']=str(args.output.resolve())
    os.environ['RUI_SEMANTIC_APPLICATION']=args.application
    os.environ['RUI_SEMANTIC_DOM_CONTROLS']='1' if args.dom_controls else '0'
    print(run_private(str(args.xvfb.resolve()),['node',str(ROOT/'demos/semantic/session-check.mjs')],timeout=120).strip())
    audit(args.output/'session.json')

if __name__=='__main__':main()
