#!/usr/bin/env python3
"""Encode exactly the RUI-rendered frames and measure size, CPU, and distortion."""
import argparse
import hashlib
import json
from pathlib import Path
import resource
import struct
import subprocess
import time
import statistics
import zlib
import numpy as np
from PIL import Image


def timed(command):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.perf_counter()
    result = subprocess.run(command, check=True, capture_output=True)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    return result, {'wallSeconds': time.perf_counter() - start, 'cpuSeconds': after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime}


def png_chunks(path):
    data = path.read_bytes()
    offsets = []
    offset = 0
    while offset < len(data):
        start = offset
        if data[offset:offset+8] != b'\x89PNG\r\n\x1a\n':
            raise ValueError('invalid PNG stream signature')
        offset += 8
        while True:
            length = struct.unpack_from('>I', data, offset)[0]
            kind = data[offset+4:offset+8]
            offset += length + 12
            if offset > len(data):
                raise ValueError('truncated PNG stream')
            if kind == b'IEND':
                break
        offsets.append({'pos': start, 'size': offset - start, 'key': True})
    return offsets


def quality(reference, decoded, width, height, count):
    expected = count * width * height * 4
    if reference.stat().st_size != expected or decoded.stat().st_size != expected:
        raise ValueError('decoded frame count/size mismatch')
    source = np.memmap(reference, dtype=np.uint8, mode='r', shape=(count, height, width, 4))
    target = np.memmap(decoded, dtype=np.uint8, mode='r', shape=source.shape)
    total_squared, edge_squared, edge_absolute, edge_bad, edge_channels = 0., 0., 0., 0, 0
    identical = True
    for a, b in zip(source, target):
        if not np.all(a[..., 3] == 255) or not np.all(b[..., 3] == 255):
            raise ValueError('nonopaque reference or decoder output')
        error = a[..., :3].astype(np.float32) - b[..., :3].astype(np.float32)
        total_squared += np.square(error).sum(dtype=np.float64)
        mask = a[..., :3].max(axis=2) > 16
        padded = np.pad(mask, 1)
        mask = np.logical_or.reduce([padded[y:y+height, x:x+width] for y in range(3) for x in range(3)])
        edge = error[mask]
        edge_squared += np.square(edge).sum(dtype=np.float64)
        edge_absolute += np.abs(edge).sum(dtype=np.float64)
        edge_bad += int((np.abs(edge) > 8).sum())
        edge_channels += edge.size
        identical &= bool(np.all(error == 0))
    mse = total_squared / (count * width * height * 3)
    return {'exact': identical, 'psnrDb': None if mse == 0 else float(10 * np.log10(255**2 / mse)), 'foregroundRmse': float(np.sqrt(edge_squared / max(1, edge_channels))), 'foregroundMae': float(edge_absolute / max(1, edge_channels)), 'foregroundChannelsOver8Fraction': edge_bad / max(1, edge_channels), 'foregroundChannels': edge_channels}


def avc_codec(path):
    data = path.read_bytes()
    for i in range(len(data) - 8):
        if data[i:i+3] == b'\x00\x00\x01' and data[i+3] & 31 == 7:
            return 'avc1.' + data[i+4:i+7].hex()
    raise ValueError('missing SPS')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--runs', type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.runs <= 5:
        parser.error('runs must be 1..5')
    directory = args.directory.resolve()
    capture = json.loads((directory/'capture.json').read_text())
    fps, count = capture['fps'], capture['frames']
    commands, results = [], []
    for width, height in [(320, 240), (640, 480), (1280, 960)]:
        raw = directory/f'{width}.rgba'
        common = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'rawvideo', '-pixel_format', 'rgba', '-video_size', f'{width}x{height}', '-framerate', str(fps), '-i', str(raw), '-an']
        variants = [
            ('png', ['-c:v', 'png', '-threads', '1', '-compression_level', '3', '-pix_fmt', 'rgb24', '-f', 'image2pipe'], '.pngstream'),
            ('h264-lossless-rgb', ['-c:v', 'libx264rgb', '-preset', 'ultrafast', '-tune', 'zerolatency', '-crf', '0', '-threads', '1', '-g', str(fps), '-keyint_min', str(fps), '-sc_threshold', '0', '-bf', '0', '-pix_fmt', 'rgb24', '-f', 'h264'], '.h264'),
        ]
        for crf in [18, 28]:
            variants.append((f'h264-crf{crf}', ['-c:v', 'libx264', '-preset', 'ultrafast', '-tune', 'zerolatency', '-crf', str(crf), '-threads', '1', '-g', str(fps), '-keyint_min', str(fps), '-sc_threshold', '0', '-bf', '0', '-pix_fmt', 'yuv420p', '-f', 'h264'], '.h264'))
        for name, options, suffix in list(variants):
            if name in ['h264-lossless-rgb', 'h264-crf18']:
                fast = options.copy(); fast[fast.index('ultrafast')] = 'veryfast'
                variants.append((name + '-veryfast', fast, suffix))
        for name, options, suffix in variants:
            path = directory/f'{width}-{name}{suffix}'
            command = common + options + [str(path)]
            commands.append(command)
            encode_runs = []
            digest = None
            for _ in range(args.runs):
                _, timing = timed(command)
                current = hashlib.sha256(path.read_bytes()).hexdigest()
                if digest is not None and current != digest:
                    raise ValueError('nondeterministic encoding for identical input/settings')
                digest = current
                encode_runs.append(timing)
            encoding = {key: statistics.median(run[key] for run in encode_runs) for key in ['wallSeconds', 'cpuSeconds']}
            decoded = directory/f'{width}-{name}.decoded.rgba'
            decode_command = ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-threads', '1']
            if name == 'png':
                decode_command += ['-f', 'image2pipe', '-framerate', str(fps)]
            decode_command += ['-i', str(path), '-threads', '1', '-pix_fmt', 'rgba', '-f', 'rawvideo', str(decoded)]
            commands.append(decode_command)
            _, decoding = timed(decode_command)
            distortion = quality(raw, decoded, width, height, count)
            if (name == 'png' or name.startswith('h264-lossless-rgb')) and not distortion['exact']:
                raise ValueError('lossless path failed exact pixel comparison')
            if name == 'png':
                packets = png_chunks(path)
                codec = None
            else:
                probe = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-show_packets', '-show_entries', 'packet=pos,size,flags', '-of', 'json', str(path)]))
                packets = [{'pos': int(p['pos']), 'size': int(p['size']), 'key': 'K' in p['flags']} for p in probe['packets']]
                codec = avc_codec(path)
            if len(packets) != count:
                raise ValueError(f'packet/frame mismatch for {name}: {len(packets)} != {count}')
            (path.with_suffix(path.suffix + '.json')).write_text(json.dumps({'codec': codec, 'fps': fps, 'packets': packets}) + '\n')
            reference_frame = np.memmap(raw, dtype=np.uint8, mode='r', shape=(count,height,width,4))[count//2]
            decoded_frame = np.memmap(decoded, dtype=np.uint8, mode='r', shape=(count,height,width,4))[count//2]
            Image.fromarray(reference_frame.copy()).save(directory/f'{width}-reference.png')
            Image.fromarray(decoded_frame.copy()).save(directory/f'{width}-{name}.png')
            decoded.unlink()
            result = {'width': width, 'height': height, 'codec': name, 'bytes': path.stat().st_size, 'bytesPerFrame': path.stat().st_size / count, 'frameBytes': [packet['size'] for packet in packets], 'firstFrameBytes': packets[0]['size'], 'keyFrames': sum(packet['key'] for packet in packets), 'bitsPerSecond': path.stat().st_size * 8 * fps / count, 'encode': encoding, 'encodeRuns': encode_runs, 'decodeFfmpeg': decoding, 'quality': distortion, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
            results.append(result)
            print(json.dumps({key: result[key] for key in ['width','codec','bitsPerSecond','quality']}), flush=True)
    data = (directory/'publications.rui').read_bytes()
    offset, compressed, compression_ms = 0, [], []
    while offset < len(data):
        length = struct.unpack_from('>I', data, offset)[0]
        offset += 4
        start = time.perf_counter()
        compressed.append(len(zlib.compress(data[offset:offset+length], level=6)))
        compression_ms.append((time.perf_counter() - start) * 1000)
        offset += length
    report = {'results': results, 'commands': commands, 'ffmpegVersion': subprocess.check_output(['ffmpeg', '-version'], text=True).splitlines()[0], 'ruiIndependentZlib6Bytes': compressed, 'ruiCompressionImplemented': False, 'ruiIndependentZlib6Ms': compression_ms}
    (directory/'codecs.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
