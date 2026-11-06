#!/usr/bin/env python3
"""Paired low-latency H.264 sweep and exact changed-frame PNG baseline."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import numpy as np
from PIL import Image
import io
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('game_codecs',ROOT/'demos/games/bench/codecs.py')
codec=importlib.util.module_from_spec(spec);spec.loader.exec_module(codec)

def main():
    p=argparse.ArgumentParser();p.add_argument('directory',type=Path);args=p.parse_args();directory=args.directory.resolve()
    capture=json.loads((directory/'capture.json').read_text());fps=capture['fps'];count=capture['frames'];results=[]
    for raw in sorted(directory.glob('[0-9]*.rgba')):
        width=int(raw.stem);height=width*3//4
        for name,encoder,crf,pix in [('h264-crf18','libx264','18','yuv420p'),('h264-crf28','libx264','28','yuv420p'),('h264-lossless','libx264rgb','0','rgb24')]:
            target=directory/f'{width}-{name}.h264';decoded=directory/f'{width}-{name}.decoded'
            command=['ffmpeg','-hide_banner','-loglevel','error','-y','-f','rawvideo','-pixel_format','rgba','-video_size',f'{width}x{height}','-framerate',str(fps),'-i',str(raw),'-an','-c:v',encoder,'-preset','veryfast','-tune','zerolatency','-crf',crf,'-threads','1','-g',str(fps),'-keyint_min',str(fps),'-sc_threshold','0','-bf','0','-pix_fmt',pix,'-f','h264',str(target)]
            _,encode=codec.timed(command)
            _,decode=codec.timed(['ffmpeg','-hide_banner','-loglevel','error','-y','-threads','1','-i',str(target),'-threads','1','-pix_fmt','rgba','-f','rawvideo',str(decoded)])
            quality={k:v for k,v in codec.quality(raw,decoded,width,height,count).items() if k in ['exact','psnrDb']}
            if name=='h264-lossless':assert quality['exact']
            probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_packets','-show_entries','packet=size,flags','-of','json',str(target)]))['packets'];assert len(probe)==count
            row={'width':width,'height':height,'codec':name,'bytes':target.stat().st_size,'firstFrameBytes':int(probe[0]['size']),'frameBytes':[int(x['size']) for x in probe],'mbps':target.stat().st_size*8/capture['seconds']/1e6,'encode':encode,'decodeFfmpeg':decode,'quality':quality,'sha256':hashlib.sha256(target.read_bytes()).hexdigest(),'command':command}
            results.append(row);decoded.unlink();print(width,name,row['mbps'],flush=True)
        # Lossless dirty-rectangle baseline includes one 16-byte rectangle header per change.
        source=np.memmap(raw,dtype=np.uint8,mode='r',shape=(count,height,width,4));prior=np.zeros_like(source[0]);sizes=[]
        for image in source:
            changed=np.any(image!=prior,axis=2);ys,xs=np.where(changed)
            if len(xs):
                area=image[ys.min():ys.max()+1,xs.min():xs.max()+1];buffer=io.BytesIO();Image.fromarray(area).save(buffer,format='PNG',compress_level=3);sizes.append(len(buffer.getvalue())+16)
                prior[ys.min():ys.max()+1,xs.min():xs.max()+1]=np.asarray(Image.open(io.BytesIO(buffer.getvalue())).convert('RGBA'))
            else:sizes.append(0)
            assert np.array_equal(prior,image), "dirty rectangle reconstruction differs"
        results.append({'width':width,'height':height,'codec':'dirty-rectangle-png','bytes':sum(sizes),'firstFrameBytes':sizes[0],'frameBytes':sizes,'mbps':sum(sizes)*8/capture['seconds']/1e6,'scope':'one changed bounding rectangle per frame; exact RGBA; 16-byte rectangle header; unchanged frames omitted'})
        Image.fromarray(source[count//2].copy()).save(directory/f'{width}-reference.png')
    (directory/'codecs.json').write_text(json.dumps({'results':results,'ffmpeg':subprocess.check_output(['ffmpeg','-version'],text=True).splitlines()[0]},indent=2)+'\n')
if __name__=='__main__':main()
