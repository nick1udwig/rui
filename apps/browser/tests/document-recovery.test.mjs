import test from 'node:test';
import assert from 'node:assert/strict';
import { DocumentMediaView } from '../src/document-media.mjs';
import { DocumentMedia } from '../document-media.mjs';
import { documentRequest } from '../src/document-request.mjs';

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const node = { id: 7, tag: 'video', mediaReady: true };
const generation = 'a'.repeat(32);
const key = `${generation}:7`;
const player = () => ({ srcObject: null, paused: true, readyState: 2, play() { this.paused = false; return Promise.resolve(); } });
class Peer {
  connectionState = 'new';
  close() { this.connectionState = 'closed'; this.onconnectionstatechange?.(); }
}

test('media recovery isolates retired sessions, retries playback, and bounds failed opens', async t => {
  const original = globalThis.RTCPeerConnection;
  globalThis.RTCPeerConnection = Peer;
  t.after(() => { if (original) globalThis.RTCPeerConnection = original; else delete globalThis.RTCPeerConnection; });
  await t.test('late failure cannot close a replacement stream', async () => {
    const pending = [], errors = [];
    const media = new DocumentMediaView((_, request) => request.op === 'open' ? new Promise((resolve, reject) => pending.push({ resolve, reject })) : Promise.resolve(), error => errors.push(error));
    try {
      media.begin(); media.attach(node, player(), generation, []);
      await delay(0);
      media.close(key);
      media.attach(node, player(), generation, []);
      const replacement = media.streams.get(key);
      pending[0].reject(Error('Old offer failed'));
      await delay(0);
      assert.equal(media.streams.get(key), replacement);
      assert.equal(replacement.closed, false);
      assert.deepEqual(errors, []);
    } finally { media.dispose(); }
  });
  await t.test('a replacement DOM element receives a new stream', async () => {
    const media = new DocumentMediaView(() => new Promise(() => {}), () => {});
    try {
      media.begin(); media.attach(node, player(), generation, []);
      const previous = media.streams.get(key), replacement = player();
      media.attach(node, replacement, generation, []);
      assert.equal(previous.closed, true);
      assert.equal(media.streams.get(key).element, replacement);
    } finally { media.dispose(); }
  });
  await t.test('interrupted sound activation retries the current player', async () => {
    const errors = [], element = player();
    let attempts = 0;
    element.play = () => {
      if (++attempts === 1) return Promise.reject(new DOMException('The play() request was interrupted by a new load request', 'AbortError'));
      element.paused = false; return Promise.resolve();
    };
    const media = new DocumentMediaView(() => new Promise(() => {}), error => errors.push(error));
    try {
      media.begin(); media.attach(node, element, generation, []);
      element.srcObject = {};
      media.enableSound();
      await delay(450);
      assert.equal(element.paused, false);
      assert.equal(attempts, 2);
      assert.deepEqual(errors, []);
    } finally { media.dispose(); }
  });
  await t.test('failed opens back off while document polls continue', async () => {
    let opens = 0;
    const media = new DocumentMediaView(async (_, request) => { if (request.op === 'open') { opens++; throw Error('Network unavailable'); } }, () => {});
    try {
      const element = player();
      for (let index = 0; index < 12; index++) {
        media.begin(); media.attach(node, element, generation, []); media.finish();
        await delay(50);
      }
      assert.ok(opens > 0 && opens <= 2, `Expected bounded retries, got ${opens}`);
    } finally { media.dispose(); }
  });
});

test('document requests time out, cancel their fetch, and ignore late results', async () => {
  let signal, complete;
  const pending = documentRequest((_, __, ___, received) => {
    signal = received;
    return new Promise(resolve => { complete = resolve; });
  }, 'document', {}, undefined, 20);
  await assert.rejects(pending, /timed out/);
  assert.equal(signal.aborted, true);
  complete({ stale: true });
  const controller = new AbortController();
  controller.abort();
  let called = false;
  await assert.rejects(documentRequest(() => { called = true; }, 'document', {}, controller.signal), { name: 'AbortError' });
  assert.equal(called, false);
});

test('host media counts pending opens and cannot resurrect a retired session', async () => {
  const pending = [], closed = [];
  const source = { generation, key: 'test', frame: { evaluate: async (_, args) => {
    if (args.node !== undefined) return new Promise(resolve => pending.push({ token: args.token, resolve }));
    closed.push(args.token);
  } } };
  const media = new DocumentMedia(source);
  const opens = Array.from({ length: 8 }, () => media.open(7, generation));
  const settled = Promise.allSettled(opens);
  assert.equal(media.sessions.size, 8);
  await assert.rejects(media.open(7, generation), /scope rejected/);
  await media.dispose();
  for (const entry of pending) entry.resolve({ type: 'offer', sdp: 'test' });
  assert.ok((await settled).every(result => result.status === 'rejected'));
  assert.equal(media.sessions.size, 0);
  assert.equal(new Set(closed).size, 8);
});
