import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher } from '../src/document-channel.mjs';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';

const urls = process.argv.slice(2);
if (!urls.length) throw Error('Supply public site URLs to probe');
const modules = new Map(await Promise.all(['document-renderer', 'document-media'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
const server = createServer((req, res) => {
  if (modules.has(req.url)) { res.setHeader('Content-Type', 'text/javascript'); res.end(modules.get(req.url)); }
  else { res.setHeader('Content-Type', 'text/html'); res.end('<style>html,body{margin:0}iframe{border:0;width:100vw;height:100dvh}</style><iframe></iframe>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
await mkdir(new URL('../test-output/', import.meta.url), { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || '/usr/bin/chromium', headless: true, chromiumSandbox: true });
try {
  for (const url of urls) {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const viewer = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const source = new BrowserDocument(page);
    let stage = "capture";
    try {
      await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 20000 });
      await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});
      await page.waitForFunction(() => [...document.querySelectorAll('h1')].some(n => n.textContent.trim()), null, { timeout: 15000 });
      const start = performance.now();
      let snapshot = await source.capture();
      const captureMs = Math.round(performance.now() - start);
      const publisher = new DocumentPublisher();
      const packet = publisher.publish(snapshot);
      const nextStart = performance.now();
      snapshot = await source.capture();
      const followup = publisher.publish(snapshot);
      const updateMs = Math.round(performance.now() - nextStart);
      await viewer.exposeFunction('hostAction', command => source.dispatch(command));
      await viewer.exposeFunction('mediaApi', async (_, request) => {
        const adapter = source.routed(request.frames);
        if (request.op === 'open') return adapter.media.open(request.node, request.generation);
        if (request.op === 'close') return adapter.media.close(request.token);
        return adapter.media.update(request.token, request.answer);
      });
      await viewer.goto(`http://127.0.0.1:${server.address().port}/`);
      await viewer.evaluate(async snapshot => {
        const { DocumentRenderer } = await import('/document-renderer.mjs');
        const { DocumentMediaView } = await import('/document-media.mjs');
        globalThis.failures = [];
        globalThis.media = new DocumentMediaView(globalThis.mediaApi, error => failures.push(String(error)));
        globalThis.renderer = new DocumentRenderer(document.querySelector('iframe'), command => hostAction(command).catch(error => failures.push(String(error))), () => true, media);
        media.begin(); await renderer.render(snapshot); media.finish();
      }, snapshot);
      const replica = viewer.frames()[1];
      assert.equal(await replica.locator('script').count(), 0);
      const expected = (await page.locator('h1').allTextContents()).find(text => text.trim());
      assert.ok(expected && (await replica.locator('body').textContent()).includes(expected), `source heading remains selectable DOM: expected=${expected}, actual=${await replica.locator('h1').allTextContents()}`);
      const hostImages = await page.locator('img').evaluateAll(images => images.filter(n => n.naturalWidth && n.getBoundingClientRect().width > 0).length);
      await replica.locator('img[src]').evaluateAll(images => Promise.all(images.filter(n => n.getBoundingClientRect().width > 0).slice(0, 10).map(n => n.decode())));
      const renderedImages = await replica.locator('img').evaluateAll(images => images.filter(n => n.naturalWidth && n.getBoundingClientRect().width > 0).length);
      assert.ok(renderedImages >= hostImages, `decoded images: host=${hostImages}, replica=${renderedImages}`);
      let video = null;
      stage = "play";
      if (await page.locator('video').count()) {
        const play = replica.locator('.ytp-large-play-button');
        if (await page.locator('video').evaluate(n => n.paused)) {
          if (await play.isVisible()) await play.click({ timeout: 5000 });
          else await replica.locator('video').click({ timeout: 5000 });
        }
        await page.waitForFunction(() => document.querySelector('video').readyState >= 2 && !document.querySelector('video').paused && document.querySelector('video').currentTime > 1, null, { timeout: 15000 });
        const playing = await source.capture();
        await viewer.evaluate(async snapshot => { media.begin(); await renderer.render(snapshot); media.finish(); }, playing);
        await viewer.waitForFunction(() => document.querySelector('iframe').contentDocument.querySelector('video')?.readyState >= 2, null, { timeout: 15000 });
        await viewer.waitForFunction(async () => { const entry = [...media.streams.values()].find(e => e.element.localName === 'video'); return entry && [...(await entry.peer.getStats()).values()].some(s => s.type === 'inbound-rtp' && s.kind === 'audio' && s.packetsReceived > 0); }, null, { timeout: 10000 });
        video = await page.locator('video').evaluate(n => ({ ready: n.readyState, currentTime: n.currentTime, paused: n.paused }));
        assert.equal(video.paused, false);
        stage = 'pause';
        await replica.locator('.ytp-play-button').click({ timeout: 5000 });
        await page.waitForFunction(() => document.querySelector('video').paused);
        const paused = await source.capture();
        await viewer.evaluate(async snapshot => { media.begin(); await renderer.render(snapshot); media.finish(); }, paused);
        video.pauseControl = true;
        stage = 'seek';
        const progress = replica.locator('.ytp-progress-bar');
        const box = await progress.boundingBox();
        await progress.click({ position: { x: box.width * 0.15, y: box.height / 2 }, timeout: 5000 });
        await page.waitForFunction(() => document.querySelector('video').currentTime > 30);
        video.seekControl = true;
      }
      const name = new URL(url).hostname.replace(/[^a-z0-9]/g, '-');
      await viewer.screenshot({ path: new URL(`../test-output/site-${name}.png`, import.meta.url).pathname });
      console.log(JSON.stringify({ url, title: await page.title(), profile: packet.profile, nodes: packet.put.length, snapshotBytes: Buffer.byteLength(JSON.stringify(packet)), updateBytes: Buffer.byteLength(JSON.stringify(followup)), captureMs, updateMs, unsupported: snapshot.unsupported, rendered: true, hostImages, renderedImages, video, physicalDevice: false }));
    } catch (error) {
      await viewer.screenshot({ path: new URL('../test-output/site-failure.png', import.meta.url).pathname });
      const video = await page.locator('video').evaluateAll(nodes => nodes.map(n => ({ ready: n.readyState, network: n.networkState, error: n.error?.message, paused: n.paused })));
      console.log(JSON.stringify({ url, stage, actions: await viewer.evaluate(() => globalThis.failures), error: String(error), sizes: source.metrics, video, pageMessage: (await page.locator('body').innerText()).slice(0, 700) }));
      process.exitCode = 1;
    } finally { source.dispose(); await viewer.close(); await page.close(); }
  }
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
