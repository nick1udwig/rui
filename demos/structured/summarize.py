#!/usr/bin/env python3
import argparse
import gzip
import hashlib
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
def main():
    p=argparse.ArgumentParser();p.add_argument('directory',type=Path);args=p.parse_args();root=args.directory.resolve();games=[]
    assets=[*ROOT.glob('client-browser/dist/*.js'),*ROOT.glob('client-browser/wasm/*.js'),*ROOT.glob('client-browser/wasm/*.wasm')]
    for name in ['editor','scroll','chart','diagram']:
        d=root/name;capture=json.loads((d/'capture.json').read_text());row={'workload':name,'capture':capture,'sceneSha256':hashlib.sha256((d/'scenes.jsonl').read_bytes()).hexdigest()}
        for key,file in [('compression','publications-compression.json'),('wasm','publications-wasm.json'),('network','publications-network.json'),('recovery','recovery-network.json'),('delayedNetwork','delayed-network.json'),('codecs','codecs.json')]:
            if (d/file).exists():row[key]=json.loads((d/file).read_text())
        if (d/'render.json').exists():
            r=json.loads((d/'render.json').read_text());row['environment']={k:r[k] for k in ['browser','cpu','kernel','node','hashes']};row['render']=[]
            for size in r['results']:
                samples=sorted(x['totalMs'] for run in size['runs'] for x in run)
                row['render'].append({'width':size['width'],'frames':size['frames'],'rgbaSha256':size['rgbaSha256'],'totalMsP50':samples[(len(samples)-1)//2],'totalMsP95':samples[int(len(samples)*.95)-1]})
        row['applicationMbps']=row['compression']['compressed_publication_packet_bytes']*8/capture['seconds']/1e6
        if 'network' in row:row['tlsBothDirectionsMbps']=(row['network']['tlsDownBytes']+row['network']['tlsUpBytes'])*8/capture['seconds']/1e6
        games.append(row)
    result={'sourceHashes':{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [ROOT/'browser-host/src/bin/scene-trace.rs',ROOT/'transport/src/compression.rs',ROOT/'demos/structured/workloads.py',ROOT/'demos/structured/network.mjs',ROOT/'demos/structured/state.mjs']},'workloads':games,'assets':{'rawBytes':sum(p.stat().st_size for p in assets),'independentGzipBytes':sum(len(gzip.compress(p.read_bytes(),mtime=0)) for p in assets),'scope':'Sum of all browser distribution JS and WASM; not measured browser transfer, HTTP caching, Brotli or a video player bundle.'}}
    (root/'summary.json').write_text(json.dumps(result,indent=2)+'\n')
    lines=['| Workload | RUI packets Mb/s | TLS both ways Mb/s | H.264 CRF18 Mb/s | H.264 CRF28 Mb/s | Lossless H.264 Mb/s | Dirty PNG Mb/s |','| --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    for row in games:
        c={r['codec']:r['mbps'] for r in row.get('codecs',{}).get('results',[]) if r['width']==640}
        lines.append('| '+row['workload']+' | '+' | '.join(('—' if v is None else f'{v:.4f}') for v in [row['applicationMbps'],row.get('tlsBothDirectionsMbps'),*[c.get(k) for k in ['h264-crf18','h264-crf28','h264-lossless','dirty-rectangle-png']]])+' |')
    (root/'tables.md').write_text('\n'.join(lines)+'\n');print('\n'.join(lines))
if __name__=='__main__':main()
