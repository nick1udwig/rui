import test from 'node:test';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { DocumentPublisher } from '../src/document-channel.mjs';

test('transient document failure retains a visible, inactive page and recovers', async () => {
  const modules = new Map(await Promise.all(['document-view', 'document-renderer', 'document-channel', 'document-media', 'document-work'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => {
    res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html');
    res.end(modules.get(req.url) || '<section id=page></section>');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    const snapshot = { generation: 'a'.repeat(32), revision: 1, width: 390, height: 633, styles: [], root: { id: 1, tag: 'html', ns: 'http://www.w3.org/1999/xhtml', children: [{ id: 4, tag: 'head', ns: 'http://www.w3.org/1999/xhtml', children: [] }, { id: 2, tag: 'body', ns: 'http://www.w3.org/1999/xhtml', children: [{ id: 3, text: 'Visible page' }] }] } };
    snapshot.styleResources = {};
    for (let index = 0; index < 80; index++) {
      const body = `body { --resource-${index}: ${index}; }`;
      const key = createHash('sha256').update(body).digest('hex');
      snapshot.styleResources[key] = body;
      snapshot.styles.push({ resource: key });
    }
    const packet = new DocumentPublisher().publish(snapshot);
    await page.evaluate(async ({ packet, bodies }) => {
      const { DocumentView } = await import('/document-view.mjs');
      window.mode = 'ok';
      window.packet = packet;
      window.resourceCalls = 0; window.activeLoads = 0; window.maxLoads = 0;
      window.view = new DocumentView(document.querySelector('#page'), async (path, request) => {
        if (path === 'document-resource') {
          window.resourceCalls++; window.activeLoads++;
          window.maxLoads = Math.max(window.maxLoads, window.activeLoads);
          await new Promise(resolve => setTimeout(resolve, 50));
          window.activeLoads--;
          return { bodies: Object.fromEntries(request.keys.map(key => [key, bodies[key]])) };
        }
        if (window.mode === 'error') throw Error('Navigation during capture');
        return window.packet;
      }, () => {}, () => false, () => {});
      view.start();
    }, { packet, bodies: snapshot.styleResources });
    const frame = page.frameLocator('iframe');
    await frame.getByText('Visible page').waitFor();
    assert.ok(await page.evaluate(() => window.maxLoads > 1 && window.maxLoads <= 4), 'Resources load concurrently within the connection budget');
    await page.evaluate(() => window.firstSheet = document.querySelector('iframe').contentDocument.styleSheets[0]);
    await page.waitForTimeout(700);
    assert.equal(await page.evaluate(() => document.querySelector('iframe').contentDocument.styleSheets[0] === window.firstSheet), true, 'Unchanged styles retain their parsed stylesheet');
    assert.equal(await page.evaluate(() => window.resourceCalls), 3);
    await page.evaluate(() => window.mode = 'error');
    await page.waitForFunction(() => document.querySelector('#page').getAttribute('aria-busy') === 'true');
    assert.equal(await page.locator('iframe').isVisible(), true);
    assert.equal(await page.locator('iframe').evaluate(frame => frame.inert), true);
    assert.equal(await frame.locator('body').textContent(), 'Visible page');
    await page.evaluate(() => window.mode = 'ok');
    await page.waitForFunction(() => !document.querySelector('#page').hasAttribute('aria-busy'));
    assert.equal(await page.locator('iframe').evaluate(frame => frame.inert), false);
    await frame.getByText('Visible page').waitFor();
    assert.equal(await page.evaluate(() => window.resourceCalls), 3, 'Recovery reuses verified content-addressed resources');
    await page.evaluate(() => window.packet = { ...window.packet, generation: 'b'.repeat(32), revision: 2 });
    await page.waitForFunction(() => view.generation === 'b'.repeat(32));
    assert.equal(await page.evaluate(() => window.resourceCalls), 3, 'Navigation reuses identical resources advertised in the new document');
    await page.evaluate(() => view.stop());
    assert.equal(await page.evaluate(() => view.resources.size), 0, 'Ending the view clears retained resources');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});
