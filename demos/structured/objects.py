#!/usr/bin/env python3
"""Host-authored object/camera scene and an equivalent flattened projection."""
import argparse
import json
import math
from pathlib import Path
from workloads import base,label,segment

COUNT=32
SHAPES=[[(round((22+((i*7+kind*3)%13))*math.cos(i*math.tau/16)),round((22+((i*7+kind*3)%13))*math.sin(i*math.tau/16))) for i in range(16)] for kind in range(3)]
COLORS=[[0.1,0.9,0.5,1],[0.15,0.55,1,1],[0.8,0.55,0.12,1]]

def quarter(x):return round(x*4)/4

def frame(index,seconds=12,fps=25,flattened=False):
    t=min(index/fps,seconds*0.75)
    camera=(quarter(-80*math.sin(t*.35)),quarter(-50*math.sin(t*.4)))
    nodes=base([104,102]);nodes += [label(102,'ORBITAL FIELD / HOST OBJECTS',20,20),{'id':104,'children':[103],'clip':[0,48,640,432]}]
    world=[];ids=[]
    for n in range(COUNT):
        x=65+(n%8)*100;y=90+(n//8)*120
        if n<8:
            x+=quarter(35*math.sin(t+n*.7));y+=quarter(28*math.cos(t*.8+n))
        points=SHAPES[n%3];lines=[segment(a,b,COLORS[n%3]) for a,b in zip(points,points[1:]+points[:1])]
        if flattened:
            world.extend([[line[0]+x+camera[0],line[1]+y+camera[1],line[2]+x+camera[0],line[3]+y+camera[1],*line[4:]] for line in lines])
        else:
            ids.append(200+n)
            nodes += [{'id':200+n,'children':[300+n],'transform':[1,0,0,1,x,y]}, {'id':300+n,'segments':lines,'width':1.5}]
    if flattened:nodes += [{'id':103,'segments':world,'width':1.5}]
    else:nodes += [{'id':103,'children':ids,'transform':[1,0,0,1,*camera]}]
    return {'nodes':nodes,'text':'ORBITAL FIELD / HOST OBJECTS'}

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--seconds',type=int,default=12);args=p.parse_args()
    if not 2<=args.seconds<=120:p.error('seconds must be 2..120')
    for variant in ['objects','flattened']:
        out=args.output/variant;out.mkdir(parents=True,exist_ok=True)
        with (out/'scenes.jsonl').open('w') as f:
            for i in range(args.seconds*25):f.write(json.dumps(frame(i,args.seconds,flattened=variant=='flattened'),separators=(',',':'))+'\n')
        (out/'capture.json').write_text(json.dumps({'game':variant,'fps':25,'seconds':args.seconds,'frames':args.seconds*25,'scope':'authored quarter-unit geometry; 32 shapes, 8 moving objects, one moving camera; final quarter idle; no client simulation'},indent=2)+'\n')
if __name__=='__main__':main()
