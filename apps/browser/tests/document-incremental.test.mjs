import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher } from '../src/document-channel.mjs';

test('incremental captures and renders update a live region without revisiting unrelated subtrees', { timeout: 30000 }, async () => {
  const modules = new Map(await Promise.all(['document-renderer', 'document-channel'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => { res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html'); res.end(modules.get(req.url) || '<body></body>'); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  let source;
  try {
    const host = await browser.newPage(), viewer = await browser.newPage();
    const origin = `http://127.0.0.1:${server.address().port}`;
    await host.goto(origin); await viewer.goto(origin);
    await host.evaluate(() => { document.body.innerHTML = '<input id="field" value="initial"><div id="rich" contenteditable="true">Original</div>' + Array.from({ length: 20 }, (_, i) => `<section id="group-${i}">${Array.from({ length: 50 }, (_, j) => `<p id="row-${i}-${j}">Row ${i}:${j}</p>`).join('')}</section>`).join(''); });
    source = new BrowserDocument(host); const publisher = new DocumentPublisher();
    await viewer.evaluate(async () => {
      const { DocumentRenderer } = await import('/document-renderer.mjs'), { DocumentReceiver } = await import('/document-channel.mjs');
      const frame = document.createElement('iframe'); document.body.append(frame);
      window.renderer = new DocumentRenderer(frame); window.receiver = new DocumentReceiver();
      window.render = packet => renderer.render(receiver.apply(packet));
    });
    const render = async () => viewer.evaluate(packet => render(packet), publisher.publish(await source.capture()));
    const concurrent = await Promise.all([source.capture(), source.capture()]);
    assert.equal(concurrent[0], concurrent[1], "Concurrent viewers share a capture");
    await render();
    await render();
    assert.equal(source.metrics.inspectedNodes, 0);
    assert.ok(await viewer.evaluate(() => renderer.renderMetrics.visited <= 3));
    const changed = source.waitForChange(publisher.snapshot.sourceVersion);
    await host.locator('#row-0-0').evaluate(node => { node.firstChild.data = 'Changed'; });
    await Promise.race([changed, new Promise((_, reject) => setTimeout(() => reject(Error('Mutation did not wake the pending document request')), 1500))]);
    await render();
    assert.ok(source.metrics.inspectedNodes < 15, `Only dirty ancestors are inspected: ${source.metrics.inspectedNodes}`);
    assert.equal(await viewer.frameLocator('iframe').locator('#row-0-0').textContent(), 'Changed');
    assert.ok(await viewer.evaluate(() => renderer.renderMetrics.visited < 100));
    await host.locator('#group-1').evaluate(node => node.remove());
    await render();
    assert.equal(await viewer.frameLocator('iframe').locator('#group-1').count(), 0);
    assert.equal(await viewer.evaluate(() => renderer.nodes.size), publisher.records.size);
    await viewer.frameLocator('iframe').locator('#rich').evaluate(node => { node.textContent = 'Unconfirmed local edit'; });
    await render();
    assert.equal(await viewer.frameLocator('iframe').locator('#rich').textContent(), 'Original', 'Unchanged host content still reconciles local rich-text edits');
    await host.locator('#field').evaluate(input => { input.value = 'external'; });
    await host.waitForTimeout(2100);
    await render();
    assert.equal(await viewer.frameLocator('iframe').locator('#field').inputValue(), 'external', 'Periodic reconciliation catches property changes without DOM events');
    await host.evaluate(() => { document.querySelector('#group-2').attachShadow({ mode: 'open' }).innerHTML = '<b>Shadow</b>'; });
    await host.waitForTimeout(2100);
    await render();
    assert.equal(await viewer.frameLocator('iframe').locator('#group-2 b').textContent(), 'Shadow');
  } finally { source?.dispose(); await browser.close(); await new Promise(resolve => server.close(resolve)); }
});
