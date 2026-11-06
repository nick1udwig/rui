import test from 'node:test';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { DocumentPublisher } from '../src/document-channel.mjs';

test('transient document failure retains a visible, inactive page and recovers', async () => {
  const modules = new Map(await Promise.all(['document-view', 'document-renderer', 'document-channel', 'document-media', 'document-work', 'document-request', 'document-stream'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => {
    res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html');
    res.end(modules.get(req.url) || '<section id=page></section>');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.evaluate(() => { window.unhandled = []; addEventListener('unhandledrejection', event => unhandled.push(String(event.reason))); });
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
      window.failures = []; window.commands = []; window.controlled = false;
      window.view = new DocumentView(document.querySelector('#page'), async (path, request) => {
        if (path === 'document-resource') {
          window.resourceCalls++; window.activeLoads++;
          window.maxLoads = Math.max(window.maxLoads, window.activeLoads);
          await new Promise(resolve => setTimeout(resolve, 50));
          window.activeLoads--;
          return { bodies: Object.fromEntries(request.keys.map(key => [key, bodies[key]])) };
        }
        if (window.mode === 'error') throw Error('Navigation during capture');
        if (window.mode === 'limit') throw Object.assign(Error('This page exceeds the current page-view limit. Use the address bar or host browser to open another page.'), { code: 'DOCUMENT_LIMIT' });
        if (window.mode === 'hung') return new Promise((resolve, reject) => { window.hung = { resolve, reject }; });
        return window.packet;
      }, command => commands.push(command), () => controlled, error => failures.push(String(error)));
      window.mediaDisposals = 0;
      const dispose = view.media.dispose.bind(view.media);
      view.media.dispose = () => { mediaDisposals++; dispose(); };
      view.start();
    }, { packet, bodies: snapshot.styleResources });
    const frame = page.frameLocator('iframe');
    await frame.getByText('Visible page').waitFor();
    assert.ok(await page.evaluate(() => window.maxLoads > 1 && window.maxLoads <= 4), 'Resources load concurrently within the connection budget');
    await page.evaluate(() => {
      window.firstDocument = document.querySelector('iframe').contentDocument;
      window.firstSheet = firstDocument.styleSheets[0];
    });
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
    assert.equal(await page.evaluate(() => document.querySelector('iframe').contentDocument === firstDocument), true, 'Fetch recovery retains the document and live media elements');
    assert.equal(await page.evaluate(() => mediaDisposals), 0, 'Fetch failure does not destroy healthy media');
    await page.evaluate(() => mode = 'limit');
    await page.getByText('This page exceeds the current page-view limit.', { exact: false }).waitFor();
    assert.equal(await page.locator('iframe').evaluate(frame => frame.inert), true);
    await page.evaluate(() => mode = 'ok');
    await page.waitForFunction(() => !view.iframe.inert && view.status.hidden);
    for (let index = 0; index < 3; index++) {
      await page.evaluate(() => { view.requestTimeout = 100; mode = 'hung'; hung = null; });
      await page.waitForFunction(() => view.iframe.inert && !!hung);
      await page.evaluate(() => { mode = 'ok'; hung.reject(Error('Delayed network failure')); });
      await page.waitForFunction(() => !view.iframe.inert);
    }
    assert.equal(await page.evaluate(() => document.querySelector('iframe').contentDocument === firstDocument), true, 'Repeated timeouts retain the same document');
    assert.equal(await page.evaluate(() => mediaDisposals), 0);
    await page.evaluate(() => { controlled = true; });
    await frame.getByText('Visible page').click();
    assert.equal(await page.evaluate(() => commands.filter(command => command.action === 'click').length), 1, 'Input dispatch still works after repeated failures');
    await page.evaluate(() => window.packet = { ...window.packet, generation: 'b'.repeat(32), revision: 2 });
    await page.waitForFunction(() => view.generation === 'b'.repeat(32));
    assert.equal(await page.evaluate(() => window.resourceCalls), 3, 'Navigation reuses identical resources advertised in the new document');
    await page.evaluate(() => { mode = 'hung'; hung = null; view.requestTimeout = 15000; });
    await page.waitForFunction(() => !!hung);
    await page.evaluate(() => {
      const old = hung;
      view.stop(); mode = 'ok'; view.start();
      old.reject(Error('Late failure from retired view'));
    });
    await page.waitForFunction(() => view.displayedGeneration === 'b'.repeat(32) && !view.iframe.inert);
    assert.equal(await page.evaluate(() => failures.some(error => error.includes('retired view'))), false);
    await page.evaluate(() => {
      view.stop();
      const add = view.iframe.addEventListener;
      window.restoreFrameEvents = () => { view.iframe.addEventListener = add; };
      view.iframe.addEventListener = function (type, ...args) {
        if (type === 'load') { window.renderWaiting = true; return; }
        return add.call(this, type, ...args);
      };
      view.start();
    });
    await page.waitForFunction(() => window.renderWaiting);
    await page.evaluate(() => {
      view.stop(); restoreFrameEvents();
      const descriptor = Object.getOwnPropertyDescriptor(HTMLIFrameElement.prototype, 'srcdoc');
      Object.defineProperty(view.iframe, 'srcdoc', {
        configurable: true,
        get() { return descriptor.get.call(this); },
        set(value) { this.dispatchEvent(new Event('load')); descriptor.set.call(this, value); },
      });
      view.start();
    });
    await page.waitForFunction(() => view.displayedGeneration === 'b'.repeat(32) && !view.iframe.inert);
    assert.equal(await page.evaluate(() => failures.some(error => error.includes('cancelled'))), false, 'Cancelling an old frame load cannot fail the new view');
    await frame.getByText('Visible page').waitFor();
    assert.deepEqual(await page.evaluate(() => unhandled), []);
    await page.evaluate(() => view.stop());
    assert.equal(await page.evaluate(() => view.resources.size), 0, 'Ending the view clears retained resources');
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
});
