import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('replicated input submits forms, drags canvas and triggers virtualized scrolling', { timeout: 45000 }, async () => {
  const renderer = await readFile(new URL('../src/document-renderer.mjs', import.meta.url));
  const server = createServer((req, res) => {
    if (req.url === '/renderer.mjs') { res.setHeader('Content-Type', 'text/javascript'); res.end(renderer); return; }
    res.setHeader('Content-Type', 'text/html; charset=utf-8');
    res.end(`<!doctype html><style>canvas{touch-action:none}#list{height:100px;overflow:auto}#rows{height:1000px}</style><form><input id=search value=initial><button>Search</button></form><p id=result></p><canvas id=map width=240 height=100></canvas><div id=list><div id=rows>Rows 1–10</div></div><script>
    document.querySelector('form').onsubmit=e=>{e.preventDefault();document.querySelector('#result').textContent=document.querySelector('input').value};
    const map=document.querySelector('#map');let dragging=false;map.onpointerdown=()=>dragging=true;map.onpointermove=e=>{if(dragging)map.dataset.pan=String(e.offsetX)};map.onpointerup=()=>dragging=false;
    document.querySelector('#list').onscroll=e=>{if(e.target.scrollTop>100)document.querySelector('#rows').textContent='Rows 11–20'};
    </script>`);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, receiverBrowser, source;
  try {
    browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
    const host = await browser.newPage({ viewport: { width: 390, height: 844 } });
    await host.goto(`http://127.0.0.1:${server.address().port}/`);
    source = new BrowserDocument(host);
    receiverBrowser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
    const viewer = await receiverBrowser.newPage({ viewport: { width: 390, height: 844 } });
    await viewer.goto(`http://127.0.0.1:${server.address().port}/viewer`);
    const commands = [];
    let sequence = Promise.resolve();
    await viewer.exposeFunction('dispatch', command => {
      commands.push(command);
      sequence = sequence.then(() => source.dispatch(command));
      return sequence;
    });
    await viewer.evaluate(async snapshot => {
      const { DocumentRenderer } = await import('/renderer.mjs');
      const iframe = document.createElement('iframe'); iframe.style.cssText = 'width:360px;height:700px'; document.body.replaceChildren(iframe);
      globalThis.control = true;
      globalThis.errors = [];
      globalThis.renderer = new DocumentRenderer(iframe, command => dispatch(command).catch(e => errors.push(String(e))), () => control, { attach() {} });
      await renderer.render(snapshot);
    }, await source.capture());
    const replica = viewer.frames()[1];
    await replica.locator('#search').fill('Search 日本語 🙂');
    await replica.locator('#search').press('Enter');
    await host.locator('#result').filter({ hasText: 'Search 日本語 🙂' }).waitFor();
    await sequence;
    await viewer.bringToFront();
    const box = await replica.locator('#map').boundingBox();
    assert.ok(box.width >= 240 && box.height >= 100);
    await viewer.mouse.move(box.x + 20, box.y + 40);
    await viewer.mouse.down();
    await viewer.mouse.move(box.x + 180, box.y + 40, { steps: 10 });
    await viewer.mouse.up();
    await host.waitForFunction(() => Number(document.querySelector('#map').dataset.pan) > 150, null, { timeout: 5000 });
    await replica.locator('#list').evaluate(n => n.scrollTop = 200);
    await host.locator('#rows').filter({ hasText: 'Rows 11–20' }).waitFor();
    await viewer.evaluate(snapshot => renderer.render(snapshot), await source.capture());
    assert.equal(await replica.locator('#rows').textContent(), 'Rows 11–20');
    await sequence;
    assert.ok(commands.some(c => c.action === 'drag'));
    assert.ok(commands.some(c => c.key === 'Enter'));
    assert.deepEqual(await viewer.evaluate(() => errors), []);
    const before = commands.length;
    await viewer.evaluate(() => control = false);
    await replica.locator('#search').press('Enter');
    assert.equal(commands.length, before);
  } finally { source?.dispose(); await receiverBrowser?.close(); await browser?.close(); await new Promise(resolve => server.close(resolve)); }
});
