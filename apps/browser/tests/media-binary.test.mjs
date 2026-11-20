import test from 'node:test';
import assert from 'node:assert/strict';
import { mediaPackets } from '../src/document-stream.mjs';
import { streamMedia } from '../media-response.mjs';
import { createServer } from 'node:http';

const frame = (generation, chunks) => {
  const metadata = Buffer.from(JSON.stringify({ generation, chunks: chunks.map((data, i) => ({ sequence: i + 1, bytes: data.length })) }));
  const header = Buffer.alloc(4); header.writeUInt32BE(metadata.length);
  return Buffer.concat([header, metadata, ...chunks]);
};
const response = bytes => new Response(new ReadableStream({ start(controller) { for (const byte of bytes) controller.enqueue(Uint8Array.of(byte)); controller.close(); } }));
test('binary media accepts arbitrary fragmentation and rejects stale generations and oversized chunks', async () => {
  const bytes = frame('current', [Buffer.from([0, 255, 10]), Buffer.from([40, 80])]);
  const packets = mediaPackets(response(bytes), 'current', new AbortController().signal);
  assert.deepEqual((await packets.next()).value.chunks.map(c => [...c.data]), [[0, 255, 10], [40, 80]]);
  await assert.rejects(packets.next(), /ended/);
  await assert.rejects(mediaPackets(response(bytes), 'retired', new AbortController().signal).next(), /scope/);
  await assert.rejects(mediaPackets(response(frame('current', [Buffer.alloc(512 * 1024 + 1)])), 'current', new AbortController().signal).next(), /limit/);
  await assert.rejects(mediaPackets(response(bytes.subarray(0, bytes.length - 1)), 'current', new AbortController().signal).next(), /ended/);
});
test('persistent HTTP media checks authorization before each batch and terminates on revocation', async () => {
  let valid = true, sequence = 0, validations = 0, closed;
  const stopped = new Promise(resolve => { closed = resolve; });
  const server = createServer(async (_, res) => {
    try {
      await streamMedia(res, { generation: 'current', read: async () => { await new Promise(resolve => setTimeout(resolve, 20)); return { chunks: [{ sequence: ++sequence, data: Buffer.from([sequence]).toString('base64') }] }; }, validate: async () => { validations++; if (!valid) throw Error('Revoked'); } });
    } catch { res.destroy(); } finally { closed(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const result = await fetch(`http://127.0.0.1:${server.address().port}`);
    assert.equal(result.headers.get('content-encoding'), null);
    const packets = mediaPackets(result, 'current', new AbortController().signal);
    assert.equal((await packets.next()).value.chunks[0].sequence, 1);
    valid = false;
    await assert.rejects(packets.next());
    await stopped;
    assert.ok(validations >= 2);
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
});
