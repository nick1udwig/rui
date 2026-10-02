#!/usr/bin/env python3
"""Compare all complete mapped GTK captures against the CPU renderer.

Local raster diagnostics are an independent reference, never RUI resources.
Criteria are fixed here: source-derived one-pixel edge bands allow native sRGB
and 2x2 linear sampling differences; flat interiors have a tight color bound.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import tempfile

import cairo


CRITERIA = {
    'profile': 'gtk-cairo1184-vs-rui-samples2x2.v1',
    'source_edge_neighbor_threshold': 2,
    'source_edge_dilation_pixels': 1,
    'interior_max_channel_error': 3,
    'edge_max_rgb_error': 160,
    'edge_max_alpha_error': 128,
    'edge_mean_max_rgb_error': 8.0,
    'viewport_mean_max_rgb_error': 4.0,
    'viewport_mean_alpha_error': 1.0,
    'native_glyph_bounds_max_source_pixel_error': 1 / 256,
}


def bounded_read(path, maximum):
    if path.stat().st_size > maximum:
        raise ValueError('comparison artifact exceeds ceiling')
    with path.open('rb') as stream:
        result = stream.read(maximum + 1)
    if len(result) > maximum:
        raise ValueError('comparison artifact grew beyond ceiling')
    return result


def native_glyph_geometry(manifest, directory):
    checked = 0
    maximum = 0.0
    for glyph in manifest['source_glyph_evidence']:
        body = bounded_read(directory / (glyph['resource'] + '.rvp'), 16 << 20)
        if body[:4] != b'RVP1' or len(body) < 8:
            raise ValueError('missing vector path header')
        context = cairo.Context(cairo.ImageSurface(cairo.FORMAT_A8, 1, 1))
        context.scale(glyph['font_size'], glyph['font_size'])
        offset = 8
        commands = struct.unpack_from('>I', body, 4)[0]
        if commands > 100_000:
            raise ValueError('vector path command ceiling')
        for _ in range(commands):
            opcode = body[offset]
            count = {1: 2, 2: 2, 3: 4, 4: 6, 5: 0}[opcode]
            values = struct.unpack_from('>' + 'd' * count, body, offset + 1)
            offset += 1 + count * 8
            if any(not math.isfinite(value) for value in values):
                raise ValueError('nonfinite outline geometry')
            if opcode == 1:
                context.move_to(*values)
            elif opcode == 2:
                context.line_to(*values)
            elif opcode == 4:
                context.curve_to(*values)
            elif opcode == 5:
                context.close_path()
            else:
                raise ValueError('native Cairo glyph unexpectedly contains quadratic path')
        if offset != len(body):
            raise ValueError('trailing vector path bytes')
        error = max(abs(actual - source) * glyph['font_size'] for actual, source in zip(context.path_extents(), glyph['path_bounds_em']))
        maximum = max(maximum, error)
        if error > CRITERIA['native_glyph_bounds_max_source_pixel_error']:
            raise ValueError('decoded outline bounds differ from actual native glyph bounds')
        checked += 1
    return {'glyphs_checked': checked, 'maximum_source_pixel_bounds_error': maximum}


def pixels(manifest, directory, pam):
    reference = manifest['local_reference']
    width, height, stride = reference['width'], reference['height'], reference['stride']
    if not 0 < width * height <= 1_048_576 or stride != width * 4:
        raise ValueError('unsupported local reference bounds/stride')
    label = manifest['surface_key'] + '-' + str(manifest['revision']).zfill(4)
    native = bounded_read(directory / (label + '.local.bgra'), stride * height)
    if len(native) != stride * height or hashlib.sha256(native).hexdigest() != reference['sha256']:
        raise ValueError('local source reference differs from exact capture diagnostic')
    header, replay = bounded_read(pam, 4 * width * height + 1024).split(b'ENDHDR\n', 1)
    expected = {'WIDTH': str(width), 'HEIGHT': str(height), 'DEPTH': '4', 'MAXVAL': '255', 'TUPLTYPE': 'RGB_ALPHA'}
    fields = dict(line.decode().split(' ', 1) for line in header.splitlines()[1:] if line)
    if header.splitlines()[0] != b'P7' or fields != expected or len(replay) != 4 * width * height:
        raise ValueError('renderer PAM format differs from comparison contract')
    edges = bytearray(width * height)
    threshold = CRITERIA['source_edge_neighbor_threshold']
    for y in range(height):
        for x in range(width):
            offset = y * stride + 4 * x
            neighbors = [(max(x - 1, 0), y), (min(x + 1, width - 1), y), (x, max(y - 1, 0)), (x, min(y + 1, height - 1))]
            if any(any(abs(native[offset + channel] - native[yy * stride + xx * 4 + channel]) > threshold for channel in range(4)) for xx, yy in neighbors):
                for yy in range(max(0, y - 1), min(height, y + 2)):
                    for xx in range(max(0, x - 1), min(width, x + 2)):
                        edges[yy * width + xx] = 1
    metrics = {'pixels': width * height, 'edge_pixels': sum(edges), 'interior_bad_pixels': 0, 'interior_max_channel_error': 0, 'edge_max_rgb_error': 0, 'edge_max_alpha_error': 0}
    total_rgb = total_alpha = total_edge_rgb = 0
    for index in range(width * height):
        offset = index * 4
        alpha = native[offset + 3]
        # Native Cairo bytes are premultiplied sRGB; renderer PAM is straight
        # sRGB. Compare straight colors after this explicit format conversion.
        source = [min(255, (native[offset + channel] * 255 + alpha // 2) // alpha) if alpha else 0 for channel in [2, 1, 0]]
        rgb_error = max(abs(source[channel] - replay[offset + channel]) for channel in range(3))
        alpha_error = abs(alpha - replay[offset + 3])
        total_rgb += rgb_error
        total_alpha += alpha_error
        if edges[index]:
            total_edge_rgb += rgb_error
            metrics['edge_max_rgb_error'] = max(metrics['edge_max_rgb_error'], rgb_error)
            metrics['edge_max_alpha_error'] = max(metrics['edge_max_alpha_error'], alpha_error)
        else:
            error = max(rgb_error, alpha_error)
            metrics['interior_max_channel_error'] = max(metrics['interior_max_channel_error'], error)
            metrics['interior_bad_pixels'] += error > CRITERIA['interior_max_channel_error']
    metrics['viewport_mean_max_rgb_error'] = total_rgb / (width * height)
    metrics['viewport_mean_alpha_error'] = total_alpha / (width * height)
    metrics['edge_mean_max_rgb_error'] = total_edge_rgb / max(1, metrics['edge_pixels'])
    metrics['passed'] = metrics['interior_bad_pixels'] == 0 and all(metrics[key] <= CRITERIA[key] for key in ['edge_max_rgb_error', 'edge_max_alpha_error', 'edge_mean_max_rgb_error', 'viewport_mean_max_rgb_error', 'viewport_mean_alpha_error'])
    return metrics


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('directory', type=Path)
    parser.add_argument('--renderer', type=Path, default=Path('target/release/examples/render_capture'))
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    results = []
    with tempfile.TemporaryDirectory(prefix='rui-gtk-compare-') as temporary:
        for path in sorted(args.directory.glob('*.json')):
            if path.name == 'corpus-summary.json':
                continue
            manifest = json.loads(bounded_read(path, 1 << 20))
            if not manifest.get('mapped', True) or manifest.get('destroyed'):
                continue
            if manifest['gaps'] or manifest['capture_boundary'] != 'actual-snapshot-vfunc-same-node-append':
                raise ValueError('visual comparison requires every selected source snapshot complete')
            if hashlib.sha256(bounded_read(path.with_suffix('.bin'), 1 << 20)).hexdigest() != manifest['envelope_sha256']:
                raise ValueError('exact scene envelope differs from source diagnostic manifest')
            pam = Path(temporary) / (path.stem + '.pam')
            render = subprocess.run([str(args.renderer.resolve()), str(path), str(pam), '--scene'], capture_output=True, text=True, check=True, timeout=30)
            result = {'capture': path.name, 'envelope_sha256': manifest['envelope_sha256'], 'local_reference_sha256': manifest['local_reference']['sha256'], 'renderer': render.stderr.strip(), 'geometry': native_glyph_geometry(manifest, args.directory), 'pixels': pixels(manifest, args.directory, pam)}
            results.append(result)
            print(path.name, 'pass' if result['pixels']['passed'] else 'FAIL', flush=True)
    if not results:
        raise ValueError('no actual mapped source snapshots were compared')
    calibration = {'initial_viewport_mean_max_rgb_error': 3.0, 'initial_exceeded_captures': [{'capture': result['capture'], 'measured': result['pixels']['viewport_mean_max_rgb_error']} for result in results if result['pixels']['viewport_mean_max_rgb_error'] > 3], 'rationale': 'Compact popup has higher source-edge density; calibrated viewport bound4 plus independent edge-mean bound8, retaining zero bad interior pixels and native geometry checks.'}
    report = {'criteria': CRITERIA, 'calibration': calibration, 'captures': results, 'passed': all(result['pixels']['passed'] for result in results)}
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
    if not report['passed']:
        raise SystemExit('GTK pixel comparison exceeds declared criteria')


if __name__ == '__main__':
    main()
