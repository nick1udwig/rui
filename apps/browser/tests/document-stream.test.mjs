import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentStream } from '../src/document-stream.mjs';

test('playback moves into buffered media after a short timestamp gap', () => {
  const player = { currentTime: 5 };
  const stream = new DocumentStream(player, new AbortController().signal);
  stream.buffer = { buffered: { length: 2, start: index => [0, 5.25][index], end: index => [5, 6.5][index] } };
  stream.catchUp();
  assert.ok(player.currentTime >= 5.25 && player.currentTime < 6.5, 'A sub-two-second gap cannot strand playback at the previous range end');
});

test('HTTPS media reads are retryable, bounded and fenced by source lifetime', { timeout: 30000 }, async () => {
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  let source;
  try {
    const host = await browser.newPage();
    await host.setContent('<canvas width=64 height=64></canvas>');
    await host.evaluate(() => {
      const context = document.querySelector('canvas').getContext('2d');
      const paint = () => { context.fillStyle = `rgb(${Math.floor(performance.now()) % 255},20,80)`; context.fillRect(0, 0, 64, 64); requestAnimationFrame(paint); }; paint();
    });
    source = new BrowserDocument(host);
    const snapshot = await source.capture();
    const canvas = snapshot.root.children.find(node => node.tag === 'body').children.find(node => node.tag === 'canvas');
    const open = () => source.media.open(canvas.id, snapshot.generation, 'https', ['video/webm;codecs=vp8']);
    const first = await open();
    assert.equal(first.transport, 'https');
    let response;
    for (let attempt = 0; attempt < 40; attempt++) {
      response = await source.media.read(first.token, 0);
      if (response.chunks.length) break;
      await host.waitForTimeout(100);
    }
    assert.ok(response.chunks.length);
    const replay = await source.media.read(first.token, 0);
    assert.deepEqual(replay.chunks.slice(0, response.chunks.length), response.chunks, 'A lost response can be fetched again before acknowledgement');
    await source.media.read(first.token, response.chunks.at(-1).sequence);
    await assert.rejects(source.media.read(first.token, 0), /cursor rejected/);
    await assert.rejects(source.media.read(first.token, 1000000), /cursor rejected/);
    await source.media.close(first.token);
    await assert.rejects(source.media.read(first.token, 0), /expired/);
    const burst = await open();
    await host.evaluate(({ key, token }) => globalThis[key].media.get(token).recorder.dispatchEvent(new BlobEvent('dataavailable', { data: new Blob([new Uint8Array(1024 * 1024)]) })), { key: source.key, token: burst.token });
    await host.waitForFunction(({ key, token }) => globalThis[key].media.get(token).sequence >= 4, { key: source.key, token: burst.token });
    const firstBatch = await source.media.read(burst.token, 0);
    assert.equal(firstBatch.chunks.reduce((total, chunk) => total + Buffer.from(chunk.data, 'base64').length, 0), 512 * 1024, 'A large encoder burst is split across bounded reads');
    const secondBatch = await source.media.read(burst.token, firstBatch.chunks.at(-1).sequence);
    assert.ok(secondBatch.chunks.length);
    await source.media.close(burst.token);
    for (const [size, count] of [[4 * 1024 * 1024 + 1, 1], [512 * 1024, 9]]) {
      const opened = await open();
      const closed = await host.evaluate(({ key, token, size, count }) => {
        const state = globalThis[key], session = state.media.get(token);
        for (let index = 0; index < count; index++) session.recorder.dispatchEvent(new BlobEvent('dataavailable', { data: new Blob([new Uint8Array(size)]) }));
        return !state.media.has(token) && session.recorder.state === 'inactive' && session.stream.getTracks().every(track => track.readyState === 'ended');
      }, { key: source.key, token: opened.token, size, count });
      assert.equal(closed, true, 'Oversized chunks and stalled-consumer queues stop capture');
      await assert.rejects(source.media.read(opened.token, 0), /expired/);
      await source.media.close(opened.token);
    }
    const retired = await open();
    await host.goto('data:text/html,<p>Next document</p>');
    await assert.rejects(source.media.read(retired.token, 0), /expired|changed/);
    await source.media.dispose();
    assert.equal(source.media.sessions.size, 0);
  } finally {
    source?.dispose();
    await browser.close();
  }
});
