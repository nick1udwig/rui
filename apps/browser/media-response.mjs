export async function streamMedia(response, { generation, read, validate }) {
  response.writeHead(200, { 'Content-Type': 'application/vnd.rui.media', 'Cache-Control': 'no-store', 'X-Accel-Buffering': 'no' });
  response.flushHeaders();
  let cursor = 0;
  const write = bytes => new Promise((resolve, reject) => {
    const done = error => { clearTimeout(timer); response.off('close', closed); response.off('error', done); response.off('drain', drained); error ? reject(error) : resolve(); };
    const closed = () => done(Error('Media reader disconnected')), drained = () => done();
    const timer = setTimeout(() => done(Error('Media reader too slow')), 5000);
    response.once('close', closed); response.once('error', done); response.once('drain', drained);
    if (response.destroyed) closed();
    else if (response.write(bytes)) done();
  });
  while (!response.destroyed) {
    const result = await read(cursor);
    await validate();
    if (response.destroyed) break;
    const bodies = result.chunks.map(chunk => Buffer.from(chunk.data, 'base64'));
    const metadata = Buffer.from(JSON.stringify({ generation, playback: result.playback, chunks: result.chunks.map((chunk, index) => ({ sequence: chunk.sequence, bytes: bodies[index].length })) }));
    const header = Buffer.alloc(4); header.writeUInt32BE(metadata.length);
    await write(Buffer.concat([header, metadata, ...bodies]));
    cursor = result.chunks.at(-1)?.sequence ?? cursor;
  }
}
