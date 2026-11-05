#!/usr/bin/env python3
"""Summarize retained samples and generate comparison figures."""
import argparse
import json
import math
from pathlib import Path
import statistics
import os
import tempfile
_CACHE = tempfile.TemporaryDirectory(prefix="rui-bench-mpl-")
os.environ.setdefault("MPLCONFIGDIR", _CACHE.name)
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
from PIL import Image, ImageDraw, ImageFont
from tables import sections


def distribution(values):
    ordered = sorted(values)
    if not ordered:
        raise ValueError('empty measurement population')
    return {'count': len(ordered), 'mean': statistics.mean(ordered), **{key: ordered[max(0, math.ceil(len(ordered) * p) - 1)] for key, p in [('p50', .5), ('p95', .95), ('p99', .99)]}}


def service(runs, key):
    return {**distribution([sample[key] for run in runs for sample in run[5:]]), 'runP95': [distribution([sample[key] for sample in run[5:]])['p95'] for run in runs]}


def summarize(directory):
    capture = json.loads((directory/'capture.json').read_text())
    publications = json.loads((directory/'publications.json').read_text())
    rendering = json.loads((directory/'render.json').read_text())
    codecs = json.loads((directory/'codecs.json').read_text())
    decoding = json.loads((directory/'decode.json').read_text())
    n, fps = capture['frames'], capture['fps']
    samples = publications['samples']
    rui_bytes = [sample['bytes'] for sample in samples]
    bitrate = sum(rui_bytes) * 8 * fps / n
    result = {'capture': capture, 'environment': {key: rendering[key] for key in ['browser','cpu','kernel','node','hashes']}, 'ffmpeg': codecs['ffmpegVersion'], 'rui': {'bytes': sum(rui_bytes), 'bitsPerSecond': bitrate, 'bytesPerFrame': distribution(rui_bytes), 'coldSnapshotBytes': rui_bytes[0], 'warmBitsPerSecond': sum(rui_bytes[1:]) * 8 * fps / (n - 1), 'sceneMutationFraction': sum(s['scene_mutation_bytes'] for s in samples[1:]) / sum(rui_bytes[1:]), 'projectBuildEncodeMs': distribution([s['project_build_encode_ms'] for s in samples[5:]]), 'independentZlib6BitsPerSecond': sum(codecs['ruiIndependentZlib6Bytes']) * 8 * fps / n, 'independentZlib6Ms': distribution(codecs['ruiIndependentZlib6Ms'])}, 'resolutions': []}
    for render in rendering['results']:
        width, height = render['width'], render['height']
        row = {'width': width, 'height': height, 'rgbaSha256': render['rgbaSha256'], 'rawRgbBitsPerSecond': width * height * 3 * 8 * fps, 'ruiClient': {key: service(render['runs'], key) for key in ['reduceMs','exportMs','drawMs','totalMs']}, 'rasters': []}
        for encoded in codecs['results']:
            if encoded['width'] != width:
                continue
            data = dict(encoded)
            data['encodeCpuMsPerFrame'] = encoded['encode']['cpuSeconds'] * 1000 / n
            data['encodeWallMsPerFrame'] = encoded['encode']['wallSeconds'] * 1000 / n
            data['ruiToRasterBytes'] = sum(rui_bytes) / encoded['bytes']
            decoded = next((d for d in decoding['results'] if d['width'] == width and d['codec'] == encoded['codec']), None)
            if decoded:
                data['browserDecodeAndBlitMs'] = {'unsupported': decoded['unsupported']} if 'unsupported' in decoded else service(decoded['runs'], 'decodeAndBlitMs')
            row['rasters'].append(data)
        result['resolutions'].append(row)
    models = []
    for mbps in [.256, 1, 5]:
        models.append({'linkMbps': mbps, 'ruiAverageSerializationMs': statistics.mean(rui_bytes) * 8 / (mbps * 1000), 'ruiP95SerializationMs': distribution(rui_bytes)['p95'] * 8 / (mbps * 1000), 'ruiPayloadFitsTargetFps': bitrate <= mbps * 1e6})
    result['serializationModel'] = models
    result['ackRttOnlyFpsUpperBound'] = [{'rttMs': ms, 'fpsUpperBound': min(fps, 1000 / ms) if ms else fps} for ms in [0,20,50,100]]
    return result


def plot(summary, output):
    names = {'rui': 'RUI vectors', 'png': 'PNG lossless', 'h264-lossless-rgb-veryfast': 'H.264 RGB lossless', 'h264-crf18-veryfast': 'H.264 CRF18', 'h264-crf28': 'H.264 CRF28 ultrafast'}
    colors = ['#167a60', '#436caa', '#8064a2', '#c57a22', '#ad494d']
    plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 10, 'axes.spines.top': False, 'axes.spines.right': False})
    fig, axes = plt.subplots(1, 2, figsize=(12, 4.5), layout='constrained', sharey=True)
    for ax, game in zip(axes, summary['games']):
        x = np.arange(3)
        for index, (key, label) in enumerate(names.items()):
            values = [game['rui']['bitsPerSecond'] / 1e6 if key == 'rui' else next(r for r in row['rasters'] if r['codec'] == key)['bitsPerSecond'] / 1e6 for row in game['resolutions']]
            ax.plot(x, values, marker='o', color=colors[index], label=label, linewidth=2)
        ax.set_xticks(x, ['320 × 240', '640 × 480', '1280 × 960']); ax.set_yscale('log'); ax.set_ylabel('Payload Mb/s (log scale)'); ax.set_title(game['capture']['game'].replace('bzone', 'Battlezone').replace('asteroid', 'Asteroids')); ax.grid(alpha=.2)
    axes[1].legend(fontsize=8); fig.savefig(output/'bandwidth.png', dpi=180); fig.savefig(output/'bandwidth.svg'); plt.close(fig)
    fig, axes = plt.subplots(1, 2, figsize=(12, 4.5), layout='constrained', sharey=True)
    for ax, game in zip(axes, summary['games']):
        x = np.arange(3)
        for key, label, color in [('rui', 'RUI reduce + full export + draw', '#167a60'), ('png','PNG decode + blit','#436caa'), ('h264-crf18-veryfast','H.264 CRF18 decode + blit','#c57a22')]:
            values = [row['ruiClient']['totalMs']['p95'] if key == 'rui' else next(r for r in row['rasters'] if r['codec'] == key)['browserDecodeAndBlitMs'].get('p95', float('nan')) for row in game['resolutions']]
            ax.plot(x, values, marker='o', label=label, color=color, linewidth=2)
        ax.axhline(1000 / game['capture']['fps'], color='#777777', ls='--', label=f"{game['capture']['fps']} fps frame budget"); ax.set_xticks(x,['320 × 240','640 × 480','1280 × 960']); ax.set_ylabel('Warm per-frame wall time p95, ms'); ax.set_title(game['capture']['game'].replace('bzone', 'Battlezone').replace('asteroid', 'Asteroids')); ax.grid(alpha=.2)
    axes[1].legend(fontsize=8); fig.savefig(output/'client-cost.png', dpi=180); fig.savefig(output/'client-cost.svg'); plt.close(fig)
    game = output/'bzone'
    if (game/'640-reference.png').exists():
        files = [('640-reference.png','Reference / lossless'),('640-h264-crf18-veryfast.png','H.264 CRF18 veryfast'),('640-h264-crf28.png','H.264 CRF28 ultrafast')]
        canvas = Image.new('RGB',(1200,220),'#f5f5f5'); draw = ImageDraw.Draw(canvas)
        font = ImageFont.truetype(matplotlib.font_manager.findfont('DejaVu Sans'), 14)
        for index, (name,label) in enumerate(files):
            image = Image.open(game/name).convert('RGB'); crop = image.crop((380,0,580,90)).resize((400,180),Image.Resampling.NEAREST)
            canvas.paste(crop,(index*400,35)); draw.text((index*400+10,10),label,fill='black',font=font)
        canvas.save(output/'quality-detail.png')
        high = Image.open(game/'1280-reference.png').convert('RGB')
        low = Image.open(game/'640-reference.png').convert('RGB').resize(high.size, Image.Resampling.BILINEAR)
        zoom = Image.new('RGB',(1600,400),'#f5f5f5'); draw = ImageDraw.Draw(zoom)
        for index, (image,label) in enumerate([(high,'Vectors rerendered at 1280'),(low,'640 raster enlarged to 1280')]):
            crop = image.crop((760,0,1160,180)).resize((800,360),Image.Resampling.NEAREST)
            zoom.paste(crop,(index*800,35)); draw.text((index*800+10,10),label,fill='black',font=font)
        zoom.save(output/'zoom-detail.png')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory',type=Path)
    args = parser.parse_args()
    output = args.directory.resolve()
    summary = {'games': [summarize(output/game) for game in ['bzone','asteroid']], 'measurement': 'paired payload and offline processing comparison; not full remote-desktop end-to-end latency'}
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    (output/'tables.md').write_text('\n\n'.join(sections(summary).values()).rstrip() + '\n')
    plot(summary, output)
    for svg in output.glob('*.svg'):
        svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines()) + '\n')
    for game in summary['games']:
        row = next(r for r in game['resolutions'] if r['width']==640)
        print(game['capture']['game'], 'RUI Mb/s',round(game['rui']['bitsPerSecond']/1e6,3))
        for raster in row['rasters']:
            print(' ',raster['codec'],round(raster['bitsPerSecond']/1e6,3),'Mb/s',round(raster['ruiToRasterBytes'],2),'RUI/raster',round(raster['encodeCpuMsPerFrame'],2),'encode CPU ms/frame')


if __name__=='__main__':
    main()
