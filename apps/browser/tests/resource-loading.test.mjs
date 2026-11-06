import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('source assets load concurrently within a budget and unchanged CSS stays cached', async () => {
  let active = 0, maximum = 0, requests = 0;
  const count = 32;
  const server = createServer((req, res) => {
    if (req.url.startsWith('/image/')) {
      requests++; active++; maximum = Math.max(maximum, active);
      setTimeout(() => {
        active--;
        res.setHeader('Content-Type', 'image/png');
        res.end(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9x8AAAAASUVORK5CYII=', 'base64'));
      }, 30);
      return;
    }
    res.setHeader('Content-Type', 'text/html');
    res.end(`<style>${Array.from({ length: 2000 }, (_, i) => `.rule-${i}{padding-left:${i % 12}px}`).join('')}</style>${Array.from({ length: count * 2 }, (_, i) => `<img src="/image/${i % count}">`).join('')}`);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  let source;
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    requests = maximum = 0;
    source = new BrowserDocument(page);
    const cold = await source.capture();
    assert.equal(requests, count, 'Repeated image URLs share one resource fetch');
    assert.ok(maximum > 1 && maximum <= 8, `Concurrent source requests: ${maximum}`);
    assert.ok(source.metrics.sourceStyleBytes > 50000);
    const warm = await source.capture();
    assert.equal(requests, count, 'Warm capture does not fetch unchanged assets');
    assert.ok(source.metrics.sourceStyleBytes < 100);
    assert.deepEqual(warm.styles, cold.styles);
    assert.deepEqual(warm.styleResources, cold.styleResources);
  } finally {
    source?.dispose();
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});
