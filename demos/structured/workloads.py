#!/usr/bin/env python3
"""Deterministic authored workloads, not captures of third-party applications."""
import argparse
import json
import math
from pathlib import Path

# A deliberately small source-owned 5x7 font; glyph geometry is in the snapshot.
FONT = dict(zip(" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789:=+-./()", [
"00000/00000/00000/00000/00000/00000/00000",
"01110/10001/10001/11111/10001/10001/10001","11110/10001/10001/11110/10001/10001/11110",
"01111/10000/10000/10000/10000/10000/01111","11110/10001/10001/10001/10001/10001/11110",
"11111/10000/10000/11110/10000/10000/11111","11111/10000/10000/11110/10000/10000/10000",
"01111/10000/10000/10111/10001/10001/01111","10001/10001/10001/11111/10001/10001/10001",
"11111/00100/00100/00100/00100/00100/11111","00111/00010/00010/00010/10010/10010/01100",
"10001/10010/10100/11000/10100/10010/10001","10000/10000/10000/10000/10000/10000/11111",
"10001/11011/10101/10101/10001/10001/10001","10001/11001/10101/10011/10001/10001/10001",
"01110/10001/10001/10001/10001/10001/01110","11110/10001/10001/11110/10000/10000/10000",
"01110/10001/10001/10001/10101/10010/01101","11110/10001/10001/11110/10100/10010/10001",
"01111/10000/10000/01110/00001/00001/11110","11111/00100/00100/00100/00100/00100/00100",
"10001/10001/10001/10001/10001/10001/01110","10001/10001/10001/10001/10001/01010/00100",
"10001/10001/10001/10101/10101/10101/01010","10001/10001/01010/00100/01010/10001/10001",
"10001/10001/01010/00100/00100/00100/00100","11111/00001/00010/00100/01000/10000/11111",
"01110/10001/10011/10101/11001/10001/01110","00100/01100/00100/00100/00100/00100/01110",
"01110/10001/00001/00010/00100/01000/11111","11110/00001/00001/01110/00001/00001/11110",
"00010/00110/01010/10010/11111/00010/00010","11111/10000/10000/11110/00001/00001/11110",
"01110/10000/10000/11110/10001/10001/01110","11111/00001/00010/00100/01000/01000/01000",
"01110/10001/10001/01110/10001/10001/01110","01110/10001/10001/01111/00001/00001/01110",
"00000/00100/00100/00000/00100/00100/00000","00000/00000/11111/00000/11111/00000/00000",
"00000/00100/00100/11111/00100/00100/00000","00000/00000/00000/11111/00000/00000/00000",
"00000/00000/00000/00000/00000/00110/00110","00001/00010/00010/00100/01000/01000/10000",
"00010/00100/01000/01000/01000/00100/00010","01000/00100/00010/00010/00010/00100/01000",
]))
WHITE = [0.7,0.8,1,1]
GREEN = [0.1,0.9,0.5,1]

def segment(a,b,color=GREEN):
    return [*a,*b,*color]

def label(id,text,x,y,color=WHITE):
    lines=[]
    for i,c in enumerate(text):
        for row,bits in enumerate(FONT[c].split('/')):
            at=0
            while at<5:
                if bits[at]=='0': at+=1; continue
                end=at+1
                while end<5 and bits[end]=='1': end+=1
                lines.append(segment((x+12*i+at*2,y+row*2),(x+12*i+end*2,y+row*2),color));at=end
    return {'id':id,'segments':lines,'width':2}

def base(children):
    return [{'id':100,'children':[101,*children]},{'id':101,'rect':[0,0,640,480],'color':[0.008,0.012,0.025,1]}]

def frame(kind,i,fps=25,seconds=12):
    # Hold the last quarter unchanged, so video can exploit idle frames too.
    t=min(i/fps,seconds*0.75)
    if kind=='editor':
        lines=[f'LINE {n:02d}  VALUE = {n*17:04d}' for n in range(18)]
        row=int(t*2)%18
        lines[row]=f'LINE {row:02d}  VALUE = {int(t*5):04d}'
        nodes=base(range(200,218))
        nodes += [label(200+n,s,24,32+n*23) for n,s in enumerate(lines)]
        return {'nodes':nodes,'text':'\n'.join(lines)}
    if kind=='scroll':
        text=[f'ROW {n:02d}  RETAINED TEXT {n*7:03d}' for n in range(48)]
        nodes=base([102]);nodes += [{'id':102,'children':[103],'clip':[12,12,616,456]}, {'id':103,'children':list(range(200,248)),'transform':[1,0,0,1,0,-t*50]}]
        nodes += [label(200+n,s,24,20+n*23) for n,s in enumerate(text)]
        return {'nodes':nodes,'text':'\n'.join(text)}
    if kind=='chart':
        nodes=base([102,103,104]);nodes += [label(102,'LIVE SIGNAL',24,20)]
        nodes += [{'id':103,'segments':[segment((40,y),(620,y),[0.06,0.09,0.14,1]) for y in range(70,440,50)],'width':1}]
        points=[(40+x*4,250+75*math.sin(x/16-t*2)+35*math.cos(x/7+t)) for x in range(146)]
        nodes += [{'id':104,'segments':[segment(a,b) for a,b in zip(points,points[1:])],'width':2}]
        return {'nodes':nodes,'text':'LIVE SIGNAL'}
    if kind=='diagram':
        nodes=base([102,103,104]);nodes += [label(102,'SERVICE MAP',24,20)]
        positions=[(90+(n%4)*145+12*math.sin(t+n),110+(n//4)*130+15*math.cos(t*0.7+n)) for n in range(12)]
        nodes += [{'id':103,'children':list(range(200,212))},{'id':104,'segments':[segment(positions[n],positions[(n+1)%12],[0.08,0.2,0.35,1]) for n in range(12)],'width':2}]
        for n,(x,y) in enumerate(positions):
            nodes += [{'id':200+n,'children':[300+n,400+n],'transform':[1,0,0,1,x,y]}, {'id':300+n,'rect':[-45,-28,90,56],'color':[0.015,0.12,0.1,1]},label(400+n,f'N{n:02d}',-18,-6,GREEN)]
        return {'nodes':nodes,'text':'SERVICE MAP'}
    raise ValueError(kind)

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',type=Path,required=True);p.add_argument('--seconds',type=int,default=12);args=p.parse_args()
    if not 2<=args.seconds<=120: p.error('seconds must be 2..120')
    for kind in ['editor','scroll','chart','diagram']:
        out=args.output/kind;out.mkdir(parents=True,exist_ok=True)
        with (out/'scenes.jsonl').open('w') as f:
            for i in range(args.seconds*25): f.write(json.dumps(frame(kind,i,seconds=args.seconds),separators=(',',':'))+'\n')
        (out/'capture.json').write_text(json.dumps({'game':kind,'fps':25,'seconds':args.seconds,'frames':args.seconds*25,'scope':'authored controlled workload; final quarter idle; source-owned 5x7 font'},indent=2)+'\n')
if __name__=='__main__':main()
