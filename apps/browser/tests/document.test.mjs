import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";
import { BrowserDocument } from "../document.mjs";

test("document replica renders responsive CSS, SVG, images and live fields without site scripts", async () => {
  const renderer = await readFile(new URL("../src/document-renderer.mjs", import.meta.url));
  let unexpected = 0;
  const server = createServer((req, res) => {
    if (req.url === "/renderer.mjs") { res.setHeader("Content-Type", "text/javascript"); res.end(renderer); return; }
    if (req.url === "/viewer") { res.end('<iframe style="width:390px;height:600px;border:0"></iframe>'); return; }
    if (req.url === "/child") { res.end('<p>Embedded content</p><button onclick="this.textContent=\'Frame clicked\'">Frame action</button>'); return; }
    if (req.url === "/pixel.png") { res.setHeader("Content-Type", "image/png"); res.end(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a9x8AAAAASUVORK5CYII=", "base64")); return; }
    if (req.url === "/leak") { unexpected++; res.end(); return; }
    res.setHeader("Content-Type", "text/html");
    res.end(`<!doctype html><style>main {display:grid;grid-template-columns:1fr 1fr} @media(max-width:500px){main{grid-template-columns:1fr}h1{color:rgb(1,2,3)}} h1::before{content:'Prefix '}</style><main><h1>Real text</h1><p>Second column</p></main><img src=/pixel.png><input id=field value=initial><input id=secret type=password value=private><svg width=30 height=30><circle cx=15 cy=15 r=10 fill=red></circle></svg><canvas></canvas><button onclick="fetch('/leak')">Button</button><script>document.querySelector('#field').value='live';</script>`);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  let source;
  try {
    browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || "/usr/bin/chromium", headless: true, chromiumSandbox: true });
    const host = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    await host.goto(origin);
    await host.evaluate(port => {
      const child = document.createElement('iframe');
      child.src = `http://localhost:${port}/child`;
      document.body.append(child);
      const host = document.createElement('x-card');
      host.attachShadow({ mode: 'open' }).innerHTML = '<style>p {color:rgb(10,20,30)}</style><p>Shadow text</p>';
      document.body.append(host);
    }, server.address().port);
    await host.frameLocator('iframe').getByText('Embedded content').waitFor();
    source = new BrowserDocument(host);
    const snapshot = await source.capture();
    assert.equal(JSON.stringify(snapshot).includes("private"), false);
    assert.equal(snapshot.unsupported[0].kind, "canvas");
    const second = await source.capture();
    assert.equal(second.root.id, snapshot.root.id);
    const viewer = await browser.newPage();
    await viewer.goto(`${origin}/viewer`);
    await viewer.evaluate(async snapshot => {
      const { DocumentRenderer } = await import('/renderer.mjs');
      globalThis.renderer = new DocumentRenderer(document.querySelector('iframe'));
      await renderer.render(snapshot);
    }, snapshot);
    const frame = viewer.frames()[1];
    assert.equal(await frame.locator('#field').inputValue(), 'live');
    assert.equal(await frame.locator('#secret').inputValue(), '');
    assert.equal(await frame.locator('h1').textContent(), 'Real text');
    assert.equal(await frame.locator('h1').evaluate(n => getComputedStyle(n).color), 'rgb(1, 2, 3)');
    assert.equal(await frame.locator('h1').evaluate(n => getComputedStyle(n, '::before').content), '"Prefix "');
    assert.equal(await frame.locator('svg circle').count(), 1);
    assert.equal(await frame.locator('x-card p').evaluate(n => getComputedStyle(n).color), 'rgb(10, 20, 30)');
    await frame.frameLocator('iframe').getByText('Embedded content').waitFor();
    await frame.locator('img').evaluate(n => n.decode());
    await frame.locator('button').click();
    assert.equal(unexpected, 0);
    assert.equal(await frame.locator('script').count(), 0);
    assert.equal(await frame.locator('[data-rui-unsupported=canvas]').count(), 1);
    const mobile = await frame.locator('main').evaluate(n => getComputedStyle(n).gridTemplateColumns.split(' ').length);
    assert.equal(mobile, 1);
    await frame.locator('#field').focus();
    await frame.locator('#field').evaluate(n => { n.value = 'draft'; n.setSelectionRange(2, 2); globalThis.savedField = n; });
    await host.locator('h1').evaluate(n => n.textContent = 'Updated');
    const update = await source.capture();
    await viewer.evaluate(snapshot => renderer.render(snapshot), update);
    assert.equal(await frame.locator('h1').textContent(), 'Updated');
    assert.equal(await frame.locator('#field').evaluate(n => n === globalThis.savedField && n === document.activeElement && n.selectionStart === 2), true);
    assert.equal(await frame.locator('#field').inputValue(), 'draft');
    const find = (node, predicate) => predicate(node) ? node : (node.children || []).map(n => find(n, predicate)).find(Boolean);
    const field = find(update.root, n => n.attrs?.id === 'field');
    await source.dispatch({ generation: update.generation, revision: update.revision, node: field.id, action: 'value', expected: 'live', value: 'Remote 🙂' });
    assert.equal(await host.locator('#field').inputValue(), 'Remote 🙂');
    await assert.rejects(source.dispatch({ generation: update.generation, revision: update.revision, node: field.id, action: 'value', expected: 'live', value: 'stale' }), /changed/);
    const button = find(update.root, n => n.tag === 'button');
    await source.dispatch({ generation: update.generation, revision: update.revision, node: button.id, action: 'click' });
    await host.waitForLoadState('networkidle');
    assert.equal(unexpected, 1);
    const embedded = find(update.root, n => n.document);
    const frameButton = find(embedded.document.root, n => n.tag === 'button');
    await source.dispatch({ frames: [{ node: embedded.id, generation: update.generation }], generation: embedded.document.generation, revision: embedded.document.revision, node: frameButton.id, action: 'click' });
    await host.frameLocator('iframe').getByText('Frame clicked').waitFor();
    await viewer.locator('iframe').evaluate(n => n.style.width = '900px');
    assert.equal(await frame.locator('main').evaluate(n => getComputedStyle(n).gridTemplateColumns.split(' ').length), 2);
    await host.reload();
    assert.notEqual((await source.capture()).generation, snapshot.generation);
    await assert.rejects(source.dispatch({ generation: update.generation, revision: update.revision, node: button.id, action: 'click' }), /Stale/);
  } finally {
    source?.dispose();
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
