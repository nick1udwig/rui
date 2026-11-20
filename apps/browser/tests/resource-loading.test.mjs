import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher } from '../src/document-channel.mjs';

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

    await page.evaluate(() => {
      document.querySelector('style').textContent = 'body { background-image: url(/image/0) }';
      document.body.replaceChildren(document.createElement('video'));
    });
    const [cssImage] = Object.keys(warm.resources);
    const viewer = new DocumentPublisher(), otherViewer = new DocumentPublisher();
    viewer.publish(warm);
    let oldPoster;
    for (const byte of [1, 2]) {
      const poster = 'data:image/png;base64,' + Buffer.alloc(3 * 1024 * 1024 + 128, byte).toString('base64');
      await page.locator('video').evaluate((node, value) => { node.poster = value; }, poster);
      const snapshot = await source.capture();
      assert.equal(Object.keys(snapshot.resources).length, 2);
      assert.ok(Object.values(snapshot.styleResources)[0].includes('rui-resource:' + cssImage));
      const newPoster = Object.keys(snapshot.resources).find(key => key !== cssImage);
      assert.ok(snapshot.resources[newPoster] === poster);
      const patch = viewer.publish(snapshot);
      if (oldPoster) {
        assert.deepEqual(patch.retireResources, [oldPoster]);
        assert.equal(viewer.resource(oldPoster), undefined);
        assert.ok(otherViewer.resource(oldPoster));
      } else {
        otherViewer.publish(snapshot);
      }
      oldPoster = newPoster;
    }
  } finally {
    source?.dispose();
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});
