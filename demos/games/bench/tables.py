"""Generate report tables directly from summary.json."""

def raster(row, name):
    return next(r for r in row['rasters'] if r['codec'] == name)


def sections(summary):
    bandwidth = [f"All rates are visual-payload Mb/s at {summary['games'][0]['capture']['fps']} workload frames/s.", '', '| Game | Output | Raw RGB24 | RUI | PNG | H.264 RGB lossless | H.264 CRF18 | H.264 CRF28 UF |', '| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |']
    cost = ['Warm browser wall time, p95 milliseconds per frame.', '', '| Game | Output | RUI total | RUI draw only | PNG decode/blit | H.264 CRF18 decode/blit |', '| --- | --- | ---: | ---: | ---: | ---: |']
    quality = ['Native-resolution quality; foreground error uses 8-bit RGB channel values.', '', '| Game | Encoding | Exact pixels | Full-frame PSNR, dB | Foreground RMSE | Foreground channels with error >8 |', '| --- | --- | --- | ---: | ---: | ---: |']
    host = ['', 'Host stage measurements at 640 × 480.', '', '| Game | RUI project/build/encode wall p95 | PNG CPU/frame | H.264 RGB lossless CPU/frame | H.264 CRF18 CPU/frame |', '| --- | ---: | ---: | ---: | ---: |']
    optimization = []
    for game in summary['games']:
        title = 'Battlezone' if game['capture']['game'] == 'bzone' else 'Asteroids'
        rate = game['rui']['bitsPerSecond']
        for row in game['resolutions']:
            rates = [row['rawRgbBitsPerSecond'], rate] + [raster(row, name)['bitsPerSecond'] for name in ['png','h264-lossless-rgb-veryfast','h264-crf18-veryfast','h264-crf28']]
            bandwidth.append(f"| {title} | {row['width']} × {row['height']} | " + ' | '.join(f'{value/1e6:.3f}' for value in rates) + ' |')
            costs = [row['ruiClient']['totalMs']['p95'],row['ruiClient']['drawMs']['p95']] + [raster(row, name)['browserDecodeAndBlitMs'].get('p95') for name in ['png','h264-crf18-veryfast']]
            cost.append(f"| {title} | {row['width']} × {row['height']} | " + ' | '.join('unsupported' if value is None else f'{value:.2f}' for value in costs) + ' |')
            if row['width'] == 640:
                for name in ['png','h264-lossless-rgb-veryfast','h264-crf18-veryfast','h264-crf28']:
                    r = raster(row, name); q = r['quality']; psnr = '∞' if q['psnrDb'] is None else f"{q['psnrDb']:.2f}"
                    quality.append(f"| {title} | {name} | {'Yes' if q['exact'] else 'No'} | {psnr} | {q['foregroundRmse']:.2f} | {q['foregroundChannelsOver8Fraction']*100:.1f}% |")
                values = [game['rui']['projectBuildEncodeMs']['p95']] + [raster(row,name)['encodeCpuMsPerFrame'] for name in ['png','h264-lossless-rgb-veryfast','h264-crf18-veryfast']]
                host.append(f'| {title} | ' + ' | '.join(f'{value:.3f} ms' for value in values) + ' |')
        rui = game['rui']
        optimization.extend([f"{title}: scene mutations account for **{rui['sceneMutationFraction']*100:.1f}%** of warm publication bytes.", f"Independent zlib level 6 would reduce payload from **{rate/1e6:.3f} to {rui['independentZlib6BitsPerSecond']/1e6:.3f} Mb/s**, a **{rate/rui['independentZlib6BitsPerSecond']:.1f}×** reduction, on this exact trace.", f"Its measured Python compression-call p95 was **{rui['independentZlib6Ms']['p95']:.3f} ms** per publication; browser decompression and integration costs were not measured.", ''])
    game = summary['games'][0]; native = next(r for r in game['resolutions'] if r['width']==640)
    fps = game['capture']['fps']
    network = ['Modeled native-resolution Battlezone serialization time per average frame.', '', '| Link capacity | RUI | Lossless RGB H.264 | CRF18 H.264 | Mean RUI load fits | Mean CRF18 load fits |', '| --- | ---: | ---: | ---: | --- | --- |']
    for mbps in [.256,1,5]:
        rates = [game['rui']['bitsPerSecond']] + [raster(native,name)['bitsPerSecond'] for name in ['h264-lossless-rgb-veryfast','h264-crf18-veryfast']]
        times = [rate / fps / (mbps * 1000) for rate in rates]
        network.append(f'| {mbps:g} Mb/s | ' + ' | '.join(f'{value:.1f} ms' for value in times) + f" | {'Yes' if rates[0] <= mbps*1e6 else 'No'} | {'Yes' if rates[2] <= mbps*1e6 else 'No'} |")
    bandwidth.extend(['', 'First visual payload at 640 × 480, excluding authentication and browser asset loading.', '', '| Game | RUI snapshot bytes | PNG first image bytes | Lossless RGB H.264 keyframe bytes | CRF18 H.264 keyframe bytes |', '| --- | ---: | ---: | ---: | ---: |'])
    for game in summary['games']:
        title = 'Battlezone' if game['capture']['game'] == 'bzone' else 'Asteroids'
        row = next(r for r in game['resolutions'] if r['width']==640)
        values = [game['rui']['coldSnapshotBytes']] + [raster(row,name)['firstFrameBytes'] for name in ['png','h264-lossless-rgb-veryfast','h264-crf18-veryfast']]
        bandwidth.append(f'| {title} | ' + ' | '.join(f'{value:,}' for value in values) + ' |')
    bandwidth.extend(['', 'H.264 lossless and CRF18 columns use `veryfast`; CRF28 UF uses `ultrafast`.', 'All retained encodings, including additional ultrafast cases, are listed in summary JSON.'])
    return {'BENCHMARK_TABLES':'\n'.join(bandwidth), 'COST_TABLE':'\n'.join(cost+host), 'QUALITY_TABLE':'\n'.join(quality), 'NETWORK_TABLE':'\n'.join(network), 'OPTIMIZATION_NUMBERS':'\n'.join(optimization)}
