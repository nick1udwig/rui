import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { gunzipSync } from "node:zlib";
import init, {
  Receiver,
  Session,
} from "../../../client-browser/wasm/rui_client_wasm.js";

const [source, packets] = process.argv.slice(2);
if (!source || !packets)
  throw new Error("usage: check-compression.mjs CORE_GZIP SESSION_PACKETS");
function frames(bytes) {
  const result = [];
  for (let offset = 0; offset < bytes.length;) {
    assert(offset + 4 <= bytes.length);
    const size = bytes.readUInt32BE(offset);
    offset += 4;
    assert(size > 0 && offset + size <= bytes.length);
    result.push(bytes.subarray(offset, offset + size));
    offset += size;
  }
  return result;
}
await init({
  module_or_path: await readFile(
    new URL(
      "../../../client-browser/wasm/rui_client_wasm_bg.wasm",
      import.meta.url,
    ),
  ),
});
const id = (value) => new Uint8Array(16).fill(value);
const reference = new Receiver(
  id(1),
  id(2),
  id(3),
  id(4),
  128 * 1024 * 1024,
  true,
);
const session = new Session(id(3), "fixture");
session.start(new Uint8Array(32).fill(7));
while (session.nextOutbound()) {}
const originals = frames(gunzipSync(await readFile(source)));
let count = 0;
let publicationMs = 0;
const decodeMs = [];
for (const packet of frames(await readFile(packets))) {
  const start = performance.now();
  session.receive(packet);
  while (session.nextOutbound()) {}
  publicationMs += performance.now() - start;
  if (session.revision > BigInt(count)) {
    decodeMs.push(publicationMs);
    publicationMs = 0;
    const publication = Buffer.alloc(16);
    publication.writeBigUInt64BE(BigInt(count + 1));
    reference.receiveEnvelope(publication, originals[count]);
    assert.deepEqual(session.state(), reference.state());
    count++;
  }
}
assert.equal(count, originals.length);
assert(session.capabilities().profiles.includes("transport.zstd-chunks.v1"));
const memoryPeak = session.memoryPeak;
session.close();
reference.close();
assert.equal(session.memoryUsed, 0);
assert.equal(reference.memoryUsed, 0);
session.free();
reference.free();
decodeMs.sort((a, b) => a - b);
console.log(
  JSON.stringify({
    frames: count,
    exactWasmState: true,
    releasedMemory: true,
    wasmAccountedPeakBytes: memoryPeak,
    receiveAndAckMsP50: decodeMs[Math.ceil(count * 0.5) - 1],
    receiveAndAckMsP95: decodeMs[Math.ceil(count * 0.95) - 1],
    timingScope:
      "one Node/WASM replay including negotiation; excludes state export, rendering, network and input",
  }),
);
