import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

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
    for (const [size, count] of [[512 * 1024 + 1, 1], [512 * 1024, 9]]) {
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
    await host.goto('about:blank#next');
    await assert.rejects(source.media.read(retired.token, 0), /expired|changed/);
    await source.media.dispose();
    assert.equal(source.media.sessions.size, 0);
  } finally {
    source?.dispose();
    await browser.close();
  }
});
