// Test-only wire construction, independent of the Rust decoder. The library
// remains Rust; this is a TypeScript consumer of generated declarations.
import { Receiver, ReceiveStatus } from '../pkg/rui_client_wasm.js';

export interface Fixture {
  kind: string;
  hex: string;
  accepted: boolean;
  revision: string;
  records: { id: string; version: string }[];
}

const encoder = new TextEncoder();
const scope = [1, 2, 3, 4].map(value => new Uint8Array(16).fill(value));
const budget = 16 * 1024 * 1024;
function receiver(): Receiver {
  return new Receiver(scope[0], scope[1], scope[2], scope[3], budget, false);
}
function check(condition: boolean, message: string): void {
  if (!condition) throw new Error(message);
}
function throws(action: () => unknown, message: string): void {
  let failed = false;
  try { action(); } catch { failed = true; }
  check(failed, message);
}
function concat(...parts: Uint8Array[]): Uint8Array {
  const result = new Uint8Array(parts.reduce((length, part) => length + part.length, 0));
  let offset = 0;
  for (const part of parts) { result.set(part, offset); offset += part.length; }
  return result;
}
function uint(value: bigint): Uint8Array {
  const bytes: number[] = [];
  do { bytes.push(Number(value & 127n) | (value >= 128n ? 128 : 0)); value >>= 7n; } while (value);
  return new Uint8Array(bytes);
}
function scalar(tag: number, value: bigint): Uint8Array {
  return concat(uint(BigInt(tag << 3)), uint(value));
}
function blob(tag: number, value: Uint8Array): Uint8Array {
  return concat(uint(BigInt(tag << 3 | 2)), uint(BigInt(value.length)), value);
}
function double(tag: number, value: number): Uint8Array {
  const body = new Uint8Array(8);
  new DataView(body.buffer).setFloat64(0, value, true);
  return concat(uint(BigInt(tag << 3 | 1)), body);
}
function envelope(payload: Uint8Array, kind = 1, session: Uint8Array = scope[0]): Uint8Array {
  return concat(scalar(1, 1n), blob(2, session), blob(3, scope[1]), blob(4, scope[3]), scalar(5, BigInt(kind)), blob(6, payload));
}
function publication(value: number): Uint8Array { return new Uint8Array(16).fill(value); }
function dispose(view: Receiver): void {
  view.close();
  check(view.memoryUsed === 0, 'Rust reservations remain after close');
  throws(() => view.revision, 'closed view exposed state');
  throws(() => view.reconnect(), 'closed view reconnected');
  view.free();
}
function fromHex(hex: string): Uint8Array {
  return new Uint8Array(hex.match(/../g)!.map(byte => parseInt(byte, 16)));
}
function textRecord(id: bigint, version: bigint, content: string): Uint8Array {
  const caret = BigInt([...content].length);
  return blob(4, concat(scalar(1, id), scalar(2, version), scalar(3, version), scalar(4, 1n),
    blob(5, encoder.encode(content)), blob(6, concat(scalar(1, caret), scalar(2, caret))), scalar(7, 1n), scalar(8, 1n)));
}
function sceneRecord(version: bigint, digest?: Uint8Array): Uint8Array {
  const primitive = digest
    ? blob(12, concat(blob(1, digest), blob(2, double(4, 1)), scalar(3, 1n)))
    : blob(10, new Uint8Array());
  return blob(2, concat(scalar(1, 1n), scalar(2, version), blob(4, concat(double(1, 1), double(4, 1))), double(6, 1), primitive));
}
async function pathResource(): Promise<{ bytes: Uint8Array; digest: Uint8Array }> {
  const bytes = new Uint8Array(43);
  bytes.set(encoder.encode('RVP1'));
  const data = new DataView(bytes.buffer);
  data.setUint32(4, 3);
  bytes[8] = 1; // Move(0, 0)
  bytes[25] = 2; // Line(10, 10)
  data.setFloat64(26, 10);
  data.setFloat64(34, 10);
  bytes[42] = 5; // Close
  const descriptor = new Uint8Array(20);
  const fields = new DataView(descriptor.buffer);
  fields.setUint32(0, 1); fields.setUint32(4, 1);
  fields.setBigUint64(8, BigInt(bytes.length)); fields.setUint32(16, 1);
  const hashInput = concat(encoder.encode('RUI/0.1/resource\0'), descriptor, bytes);
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', hashInput as Uint8Array<ArrayBuffer>));
  return { bytes, digest };
}

export async function runChecks(fixtures: Fixture[]) {
  const view = receiver();
  try {
    for (const [index, fixture] of fixtures.entries()) {
      let accepted = true;
      try { view.receiveEnvelope(publication(index + 5), envelope(fromHex(fixture.hex), fixture.kind === 'snapshot' ? 1 : 2)); }
      catch { accepted = false; }
      check(accepted === fixture.accepted, `native/WASM admission mismatch at prefix ${index}`);
      check(view.revision === BigInt(fixture.revision), `native/WASM revision mismatch at prefix ${index}`);
      const ids: BigUint64Array = view.recordIds();
      check(ids.length === fixture.records.length, `native/WASM record count mismatch at prefix ${index}`);
      for (const record of fixture.records) {
        check(ids.includes(BigInt(record.id)), 'missing native record');
        check(view.recordVersion(BigInt(record.id)) === BigInt(record.version), 'native/WASM clock mismatch');
      }
    }
  } finally { dispose(view); }

  // IDs and revisions above Number.MAX_SAFE_INTEGER cross the generated API
  // as BigInt, and repeated UTF-8 input/output copies preserve Unicode text.
  const wide = receiver();
  const large = (1n << 53n) + 123n;
  const text = 'A😀e\u0301אב';
  const initial = envelope(concat(scalar(1, large), blob(2, textRecord(large + 1n, large, text))));
  try {
    check(wide.receiveEnvelope(publication(32), initial) === ReceiveStatus.Applied, 'wide snapshot failed');
    check(typeof wide.revision === 'bigint' && wide.revision === large, 'revision lost precision');
    check(wide.recordIds()[0] === large + 1n, 'record ID lost precision');
    check(wide.textContent(large + 1n) === text, 'Unicode changed across binding');
    const update = envelope(concat(scalar(1, large), scalar(2, large + 1n),
      blob(3, blob(1, textRecord(large + 1n, large + 1n, 'updated😀')))), 2);
    wide.receiveEnvelope(publication(33), update);
    check(wide.revision === large + 1n && wide.textContent(large + 1n) === 'updated😀', 'wide transaction failed');
    throws(() => wide.receiveEnvelope(publication(34), update.slice(0, -1)), 'truncated wire accepted');
    throws(() => wide.receiveEnvelope(publication(35), envelope(new Uint8Array(), 1, publication(99))), 'foreign binding accepted');
    check(wide.revision === large + 1n, 'failed update mutated committed state');
    wide.disconnect();
    throws(() => wide.receiveEnvelope(publication(36), initial), 'disconnected view accepted input');
    wide.reconnect();
    check(wide.revision === large + 1n, 'reconnect lost committed state');
  } finally { dispose(wide); }
  throws(() => new Receiver(scope[0], scope[1], scope[2], scope[3], 1, false), 'tiny memory ceiling accepted');
  throws(() => new Receiver(new Uint8Array(1), scope[1], scope[2], scope[3], budget, false), 'invalid scope length accepted');

  const asset = await pathResource();
  for (const corrupt of [false, true]) {
    const resources = receiver();
    try {
      throws(() => resources.resourceOffer(asset.digest, 1, 1, 1, BigInt(asset.bytes.length)), 'unsolicited resource accepted');
      resources.receiveEnvelope(publication(40), envelope(concat(scalar(1, 1n), blob(2, sceneRecord(1n)))));
      resources.authorizeResource(asset.digest); // Explicit trusted test policy.
      const pending = resources.receiveEnvelope(publication(41), envelope(concat(scalar(1, 2n), blob(2, sceneRecord(2n, asset.digest)))));
      check(pending === ReceiveStatus.NeedResources && resources.revision === 1n, 'missing resource installed state prematurely');
      check(resources.missingResources().every((byte, index) => byte === asset.digest[index]) && resources.missingResources().length === 32, 'wrong resource request');
      resources.resourceOffer(asset.digest, 1, 1, 1, BigInt(asset.bytes.length));
      const body = asset.bytes.slice();
      if (corrupt) body[20] ^= 1;
      resources.resourceChunk(asset.digest, 0, body.slice(0, 20));
      resources.resourceChunk(asset.digest, 1, body.slice(20));
      if (corrupt) {
        throws(() => resources.resourceComplete(asset.digest), 'corrupt resource accepted');
        check(resources.revision === 1n, 'corrupt resource mutated state');
      } else {
        check(resources.resourceComplete(asset.digest) === ReceiveStatus.Applied, 'verified resource did not apply state');
        check(resources.revision === 2n && resources.missingResources().length === 0, 'resource commit incomplete');
        check(resources.resourceBytes(asset.digest).every((byte, index) => byte === asset.bytes[index]), 'verified body changed crossing binding');
      }
    } finally { dispose(resources); }
  }
  return { nativeOraclePrefixes: fixtures.length, wideIntegers: true, unicode: true, atomicResources: true, invalidInput: true, lifecycleAndBudget: true };
}
