import test from 'node:test';
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('detach releases observers, listeners, frame state, media and pending waits while the host stays open', { timeout: 30000 }, async () => {
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  let source;
  try {
    const page = await browser.newPage();
    await page.setContent('<p>Host</p><canvas width="32" height="32"></canvas><iframe srcdoc="<p>Child</p>"></iframe>');
    await page.evaluate(() => {
      const canvas = document.querySelector('canvas'), context = canvas.getContext('2d');
      const draw = () => { context.fillStyle = 'red'; context.fillRect(0, 0, 32, 32); requestAnimationFrame(draw); }; draw();
    });
    source = new BrowserDocument(page); const snapshot = await source.capture();
    const node = snapshot.root.children.find(n => n.tag === 'body').children.find(n => n.tag === 'canvas');
    const media = await source.media.open(node.id, snapshot.generation, 'https', ['video/webm;codecs=vp8'], { width: 32, level: 1 });
    const child = [...source.frames.values()][0]; assert.ok(child);
    const saved = await page.evaluate(({ key, token }) => {
      window.retiredState = globalThis[key]; window.retiredMedia = retiredState.media.get(token);
      retiredState.fullAt = performance.now(); retiredState.hasFrames = false;
      return retiredState.version;
    }, { key: source.key, token: media.token });
    const waiting = source.waitForChange(saved).then(() => 'resolved', error => error.message);
    await page.waitForFunction(key => globalThis[key].waiters.size > 0, source.key);
    await source.dispose();
    assert.equal(await waiting, 'Document detached');
    assert.equal(page.isClosed(), false);
    assert.equal(await page.evaluate(key => key in globalThis, source.key), false);
    assert.equal(await child.frame.evaluate(key => key in globalThis, child.key), false);
    assert.equal(source.media.sessions.size, 0);
    assert.equal(source.lastCapture, null);
    assert.deepEqual(await page.evaluate(() => ({ closed: retiredMedia.closed, recorder: retiredMedia.recorder.state, tracks: [...retiredMedia.stream.getTracks(), ...retiredMedia.scaled.getTracks()].every(track => track.readyState === 'ended'), waiters: retiredState.waiters.size })), { closed: true, recorder: 'inactive', tracks: true, waiters: 0 });
    const before = await page.evaluate(() => retiredState.version);
    await page.evaluate(() => { document.querySelector('p').textContent = 'Host still works'; document.querySelector('canvas').dispatchEvent(new Event('input', { bubbles: true })); });
    assert.equal(await page.evaluate(() => retiredState.version), before, 'Detached observers and event listeners no longer run');
    assert.equal(await page.locator('p').textContent(), 'Host still works');
    await source.dispose();
    await assert.rejects(source.capture(), /detached/);
    await assert.rejects(source.media.open(node.id, snapshot.generation, 'https', ['video/webm;codecs=vp8']), /detached/);
  } finally { await source?.dispose(); await browser.close(); }
});

test('a capture already in flight cannot recreate detached page state', { timeout: 30000 }, async () => {
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  try {
    const page = await browser.newPage(); await page.setContent('<p>Host</p>');
    for (let i = 0; i < 3; i++) {
      const source = new BrowserDocument(page);
      const capture = source.capture().then(() => 'published', error => error.message);
      await source.dispose();
      assert.match(await capture, /detached/);
      assert.equal(await page.evaluate(key => key in globalThis, source.key), false);
    }
    assert.equal(await page.locator('p').textContent(), 'Host');
  } finally { await browser.close(); }
});
