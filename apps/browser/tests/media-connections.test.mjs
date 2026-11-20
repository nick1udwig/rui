import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';

test('HTTP media reserves connections for document, resource, and control requests', { timeout: 20000 }, async () => {
  const files = new Map(await Promise.all(['document-media', 'document-request', 'document-stream'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const streams = new Set();
  const server = createServer((req, res) => {
    if (files.has(req.url)) { res.setHeader('Content-Type', 'text/javascript'); res.end(files.get(req.url)); }
    else if (req.url.startsWith('/stream/')) {
      streams.add(res);
      res.on('close', () => streams.delete(res));
      res.writeHead(200, { 'Content-Type': 'application/octet-stream' });
      res.write('pending media');
    } else res.end('ready');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || '/usr/bin/chromium', headless: true, chromiumSandbox: true });
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.evaluate(async () => {
      const { DocumentMediaView } = await import('/document-media.mjs');
      const { streamTypes } = await import('/document-stream.mjs');
      if (!streamTypes().length) throw Error('HTTP media support required');
      window.errors = [];
      window.media = new DocumentMediaView(() => {}, error => errors.push(String(error)));
      media.openStream = async entry => {
        const response = await fetch(`/stream/${entry.key}`, { signal: entry.controller.signal });
        entry.connected = true;
        await response.arrayBuffer();
      };
      window.players = Array.from({ length: 8 }, () => document.createElement('video'));
      window.attachAll = () => {
        media.begin();
        players.forEach((player, id) => media.attach({ id, tag: 'video', mediaReady: true }, player, 'generation', []));
        media.finish();
      };
      attachAll();
      attachAll();
    });
    await page.waitForFunction(() => [...media.streams.values()].every(entry => entry.connected));
    assert.equal(await page.evaluate(() => media.streams.size), 4);
    assert.equal(streams.size, 4);
    assert.deepEqual(await page.evaluate(async () => Promise.all(['document', 'resource', 'media-control'].map(async path => {
      const response = await fetch(`/${path}`, { signal: AbortSignal.timeout(2000) });
      return response.text();
    }))), ['ready', 'ready', 'ready']);
    await page.evaluate(() => {
      media.close('generation:0');
      media.attach({ id: 4, tag: 'video', mediaReady: true }, players[4], 'generation', []);
    });
    await page.waitForFunction(() => media.streams.get('generation:4')?.connected);
    assert.equal(await page.evaluate(() => media.streams.size), 4);
    assert.deepEqual(await page.evaluate(() => errors), []);
    await page.evaluate(() => media.dispose());
  } finally {
    await browser?.close();
    for (const response of streams) response.destroy();
    await new Promise(resolve => server.close(resolve));
  }
});
