import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher, DocumentReceiver } from '../src/document-channel.mjs';

test('large semantic pages retain visible links and load deferred sections as the phone scrolls', { timeout: 45000 }, async () => {
  const renderer = await readFile(new URL('../src/document-renderer.mjs', import.meta.url));
  const cards = Array.from({ length: 200 }, (_, index) => `<article id="card-${index}"><a href="/watch?v=${index}">Video ${index}</a><div>${'<span>Metadata </span>'.repeat(65)}</div></article>`).join('');
  const server = createServer((req, res) => {
    if (req.url === '/renderer.mjs') { res.setHeader('Content-Type', 'text/javascript'); res.end(renderer); return; }
    res.setHeader('Content-Type', 'text/html');
    if (req.url === '/viewer') { res.end('<style>body{margin:0}iframe{width:358px;height:633px;border:0}</style><iframe></iframe>'); return; }
    if (req.url.startsWith('/watch')) { res.end(`<p>Playing ${new URL(req.url, 'http://test').searchParams.get('v')}</p>`); return; }
    res.end(`<!doctype html><style>html{scrollbar-width:none}body{margin:0}main{display:grid;gap:12px}article{height:140px;box-sizing:border-box;padding:8px;border:1px solid black;overflow:hidden}#fixed button{position:fixed;right:0;top:0}</style><main>${cards}</main><div id=fixed style="margin-top:10000px;height:100px"><button>Always visible</button></div>`);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser, source;
  try {
    browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
    const host = await browser.newPage({ viewport: { width: 358, height: 633 } });
    const viewer = await browser.newPage({ viewport: { width: 358, height: 844 }, hasTouch: true, isMobile: true });
    await host.goto(origin);
    source = new BrowserDocument(host);
    const fullHeight = await host.evaluate(() => document.scrollingElement.scrollHeight);
    await host.locator('#card-120').evaluate(node => node.scrollIntoView({ block: 'center' }));
    await viewer.goto(origin + '/viewer');
    const commands = [], errors = [];
    let sequence = Promise.resolve();
    await viewer.exposeFunction('dispatch', command => {
      commands.push(command);
      sequence = sequence.then(() => source.dispatch(command)).catch(error => errors.push(error.message));
      return sequence;
    });
    await viewer.evaluate(async () => {
      const { DocumentRenderer } = await import('/renderer.mjs');
      window.renderer = new DocumentRenderer(document.querySelector('iframe'), command => { void dispatch(command); }, () => true);
    });
    const publisher = new DocumentPublisher(), receiver = new DocumentReceiver();
    const render = async () => {
      const snapshot = await source.capture();
      const received = receiver.apply(publisher.publish(snapshot));
      await viewer.evaluate(snapshot => renderer.render(snapshot), { ...received, resources: snapshot.resources, styleResources: snapshot.styleResources });
      return snapshot;
    };
    const first = await render();
    assert.ok(first.deferredNodes > 0);
    const count = node => 1 + [...(node.children || []), ...(node.shadow || [])].reduce((sum, child) => sum + count(child), 0);
    assert.ok(count(first.root) <= 20000);
    const replica = viewer.frameLocator('iframe');
    assert.equal(await replica.getByRole('link', { name: 'Video 120', exact: true }).isVisible(), true);
    assert.equal(await replica.getByRole('button', { name: 'Always visible' }).isVisible(), true, 'Visible fixed descendants keep their offscreen ancestors');
    assert.ok(Math.abs(await replica.locator('html').evaluate(node => node.scrollHeight) - fullHeight) <= 2, 'Deferred sections preserve scroll geometry');
    await viewer.waitForTimeout(150);
    assert.equal(commands.length, 0, 'Applying the host scroll position does not echo a scroll command');
    await replica.getByRole('link', { name: 'Video 120', exact: true }).tap();
    await host.waitForURL('**/watch?v=120');
    await sequence;
    assert.deepEqual(errors, []);
    await host.goto(origin);
    await render();
    assert.equal(await replica.locator('#card-180').getAttribute('data-rui-deferred'), '');
    await replica.locator('#card-180').evaluate(node => node.scrollIntoView({ block: 'center' }));
    await host.waitForFunction(() => scrollY > 25000);
    await sequence;
    const clickCount = commands.filter(command => command.action === 'click').length;
    await replica.locator('#card-180').tap();
    assert.equal(commands.filter(command => command.action === 'click').length, clickCount, 'Empty deferred sections do not activate unseen host controls');
    const second = await render();
    await replica.getByRole('link', { name: 'Video 180', exact: true }).waitFor();
    assert.equal(await replica.locator('#card-180').getAttribute('data-rui-deferred'), null);
    assert.ok(Math.abs(await replica.locator('html').evaluate(node => node.scrollHeight) - fullHeight) <= 2);
    await host.evaluate(() => {
      const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      svg.id = 'over-budget'; svg.style = 'position:fixed;top:0;width:100px;height:100px';
      svg.innerHTML = '<path d="M0 0L10 10"/>'.repeat(21000);
      document.body.append(svg);
    });
    await assert.rejects(source.capture(), error => error.code === 'DOCUMENT_LIMIT' && /Visible document limit/.test(error.message));
    await host.locator('#over-budget').evaluate(node => node.remove());
    const find = node => node.tag === 'a' && node.attrs?.href === '/watch?v=180' ? node : (node.children || []).map(find).find(Boolean);
    await source.dispatch({ generation: second.generation, revision: second.revision, node: find(second.root).id, action: 'click' });
    await host.waitForURL('**/watch?v=180');
    assert.deepEqual(errors, []);
  } finally {
    source?.dispose();
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
