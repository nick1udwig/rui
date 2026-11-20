import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('media scales to the displayed region and suspends offscreen picture capture', { timeout: 30000 }, async () => {
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  let source;
  try {
    const page = await browser.newPage();
    await page.setContent('<canvas width="1600" height="900"></canvas>');
    await page.evaluate(() => {
      const canvas = document.querySelector('canvas'), context = canvas.getContext('2d');
      const draw = () => { context.fillStyle = `hsl(${performance.now() / 10},100%,50%)`; context.fillRect(0, 0, 1600, 900); requestAnimationFrame(draw); }; draw();
    });
    source = new BrowserDocument(page);
    const snapshot = await source.capture();
    const node = snapshot.root.children.find(n => n.tag === 'body').children.find(n => n.tag === 'canvas');
    const opened = await source.media.open(node.id, snapshot.generation, 'https', ['video/webm;codecs=vp8'], { width: 360, level: 1 });
    assert.deepEqual(opened.encoding, { width: 360, height: 202, fps: 30, bitrate: 1200000 });
    let cursor = 0;
    const read = async () => { const result = await source.media.read(opened.token, cursor, true); cursor = result.chunks.at(-1)?.sequence ?? cursor; return result.chunks.reduce((n, c) => n + c.data.length, 0); };
    let bytes = 0;
    for (let i = 0; i < 5; i++) bytes += await read();
    assert.ok(bytes > 0);
    await source.media.visibility(opened.token, false);
    for (let i = 0; i < 4; i++) await read();
    assert.equal(await read(), 0, 'A hidden picture does not keep generating encoded frames');
    await source.media.visibility(opened.token, true);
    let resumed = 0;
    for (let i = 0; i < 4 && !resumed; i++) resumed += await read();
    assert.ok(resumed > 0);
    await source.media.close(opened.token);
    const economy = await source.media.open(node.id, snapshot.generation, 'https', ['video/webm;codecs=vp8'], { width: 1600, level: 0 });
    assert.equal(economy.encoding.width, 480); assert.equal(economy.encoding.fps, 15); assert.equal(economy.encoding.bitrate, 500000);
    await source.media.close(economy.token);
    await assert.rejects(source.media.open(node.id, snapshot.generation, 'https', ['video/webm;codecs=vp8'], { width: 50000, level: 0 }), /quality rejected/);
  } finally { source?.dispose(); await browser.close(); }
});

test('sustained decoder pressure lowers quality once and preserves the chosen level', async () => {
  const { DocumentMediaView } = await import('../src/document-media.mjs');
  const view = new DocumentMediaView(async () => ({}), error => { throw error; });
  view.visible = () => true; view.quality = () => ({ width: 360, level: 1 });
  let frames = 0;
  const element = { localName: 'video' };
  const entry = { key: 'fixture', token: 'fixture', startedAt: Date.now() - 20000, element, quality: { width: 360, level: 1 }, visible: true, controller: new AbortController(), player: { buffered: { length: 0 }, getVideoPlaybackQuality: () => ({ totalVideoFrames: frames += 30, droppedVideoFrames: frames / 3 }), remove() {} } };
  view.streams.set(entry.key, entry);
  for (let i = 0; i < 3; i++) { await view.adapt(entry); clearTimeout(entry.adaptation); }
  assert.equal(entry.closed, true);
  assert.deepEqual(view.qualities.get(entry.key), { width: 360, level: 0 });
  view.dispose();
});
