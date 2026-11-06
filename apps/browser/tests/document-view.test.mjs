import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { DocumentPublisher } from '../src/document-channel.mjs';

test('transient document failure retains a visible, inactive page and recovers', async () => {
  const modules = new Map(await Promise.all(['document-view', 'document-renderer', 'document-channel', 'document-media'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => {
    res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html');
    res.end(modules.get(req.url) || '<section id=page></section>');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    const snapshot = { generation: 'a'.repeat(32), revision: 1, width: 390, height: 633, styles: [], root: { id: 1, tag: 'html', ns: 'http://www.w3.org/1999/xhtml', children: [{ id: 2, tag: 'body', ns: 'http://www.w3.org/1999/xhtml', children: [{ id: 3, text: 'Visible page' }] }] } };
    const packet = new DocumentPublisher().publish(snapshot);
    await page.evaluate(async packet => {
      const { DocumentView } = await import('/document-view.mjs');
      window.mode = 'ok';
      window.view = new DocumentView(document.querySelector('#page'), async () => {
        if (window.mode === 'error') throw Error('Navigation during capture');
        return packet;
      }, () => {}, () => false, () => {});
      view.start();
    }, packet);
    const frame = page.frameLocator('iframe');
    await frame.getByText('Visible page').waitFor();
    await page.evaluate(() => window.mode = 'error');
    await page.waitForFunction(() => document.querySelector('#page').getAttribute('aria-busy') === 'true');
    assert.equal(await page.locator('iframe').isVisible(), true);
    assert.equal(await page.locator('iframe').evaluate(frame => frame.inert), true);
    assert.equal(await frame.locator('body').textContent(), 'Visible page');
    await page.evaluate(() => window.mode = 'ok');
    await page.waitForFunction(() => !document.querySelector('#page').hasAttribute('aria-busy'));
    assert.equal(await page.locator('iframe').evaluate(frame => frame.inert), false);
    await frame.getByText('Visible page').waitFor();
    await page.evaluate(() => view.stop());
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});
