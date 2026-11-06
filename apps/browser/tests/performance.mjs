import assert from 'node:assert/strict';
import { streamMedia } from '../media-response.mjs';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';
import { DocumentPublisher } from '../src/document-channel.mjs';
import { resourceReply } from '../document-resources.mjs';

let apiHandler;
const server = createServer(async (req, res) => {
  if (req.url.startsWith('/api/')) {
    try {
      let body = ''; for await (const chunk of req) body += chunk;
      const result = await apiHandler(req.url.slice(5), JSON.parse(body), res);
      if (result !== undefined) { res.setHeader('Content-Type', 'application/json'); res.end(JSON.stringify(result)); }
    } catch (error) { if (res.headersSent) res.destroy(); else { res.writeHead(500); res.end(JSON.stringify({ error: error.message })); } }
    return;
  }
  if (/^\/[a-z-]+\.mjs$/.test(req.url)) {
    try { res.setHeader('Content-Type', 'text/javascript'); res.end(await readFile(new URL(`../src${req.url}`, import.meta.url))); }
    catch { res.writeHead(404); res.end(); }
  } else { res.setHeader('Content-Type', 'text/html'); res.end('<body></body>'); }
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true, args: ['--autoplay-policy=no-user-gesture-required'] });
  for (const kind of ['idle', 'dashboard', 'canvas', 'video']) {
    const host = await browser.newPage({ viewport: { width: 390, height: 633 } });
    const viewer = await browser.newPage({ viewport: { width: 390, height: 844 } });
    const origin = `http://127.0.0.1:${server.address().port}`;
    await host.goto(origin); await viewer.goto(origin);
    await host.evaluate(async kind => {
      document.body.innerHTML = '<button id="action">Act</button><output id="result">Ready</output>' + Array.from({ length: 1500 }, (_, i) => `<p>Row ${i}: <span>0</span></p>`).join('');
      document.querySelector('button').onclick = () => { document.querySelector('output').textContent = 'Done'; };
      if (kind === 'dashboard') setInterval(() => { document.querySelector('span').textContent = String(performance.now()); }, 16);
      if (['canvas', 'video'].includes(kind)) {
        const canvas = document.createElement('canvas'); canvas.width = 1280; canvas.height = 720; canvas.style.width = '340px';
        const ctx = canvas.getContext('2d');
        const draw = () => { ctx.fillStyle = '#234'; ctx.fillRect(0, 0, 1280, 720); ctx.fillStyle = '#fc2'; ctx.fillRect(performance.now() / 3 % 1100, 100, 180, 400); requestAnimationFrame(draw); }; draw();
        if (kind === 'canvas') document.body.prepend(canvas);
        else {
          const recorder = new MediaRecorder(canvas.captureStream(30), { mimeType: 'video/webm;codecs=vp8' }), chunks = [];
          recorder.ondataavailable = e => chunks.push(e.data);
          const done = new Promise(resolve => recorder.onstop = resolve); recorder.start(); await new Promise(resolve => setTimeout(resolve, 1500)); recorder.stop(); await done;
          const video = document.createElement('video'); video.style.width = '340px'; video.loop = video.muted = true; video.src = URL.createObjectURL(new Blob(chunks)); document.body.prepend(video); await video.play();
        }
      }
    }, kind);
    const source = new BrowserDocument(host), publisher = new DocumentPublisher();
    let captureMs = 0, payloadBytes = 0;
    apiHandler = async (path, data, response) => {
      let result;
      if (path === 'document') { result = publisher.publish(await source.capture()); captureMs += source.metrics.captureMs; }
      else if (path === 'document-resource') result = resourceReply(source, data);
      else if (data.op === 'open') result = { ...await source.media.open(data.node, data.generation, data.transport, data.types, data.quality), delivery: 'binary' };
      else if (data.op === 'stream') {
        const write = response.write.bind(response); response.write = bytes => { payloadBytes += bytes.length; return write(bytes); };
        await streamMedia(response, { generation: source.generation, read: after => source.media.read(data.token, after, true), validate: async () => {} }); return;
      }
      else if (data.op === 'visibility') { await source.media.visibility(data.token, data.visible); result = {}; }
      else if (data.op === 'read') result = await source.media.read(data.token, data.after);
      else if (data.op === 'close') { await source.media.close(data.token); result = {}; }
      else { await source.media.update(data.token, data.answer); result = {}; }
      payloadBytes += Buffer.byteLength(JSON.stringify(result)); return result;
    };
    await viewer.exposeFunction('dispatch', command => source.dispatch(command));
    const sessions = await Promise.all([host, viewer].map(async page => { const cdp = await page.context().newCDPSession(page); await cdp.send('Performance.enable'); return cdp; }));
    const sample = async () => Promise.all(sessions.map(async cdp => Object.fromEntries((await cdp.send('Performance.getMetrics')).metrics.map(m => [m.name, m.value]))));
    await viewer.evaluate(async () => {
      const { DocumentView } = await import('/document-view.mjs');
      const api = async (path, data, auth, signal) => { const response = await fetch(`/api/${path}`, { method: 'POST', body: JSON.stringify(data), signal }); return response.headers.get('content-type') === 'application/vnd.rui.media' ? response : response.json(); };
      window.view = new DocumentView(document.body, api, dispatch, () => true, error => console.error(error)); view.start();
    });
    await viewer.frameLocator('iframe').locator('#action').waitFor();
    await viewer.waitForTimeout(1500);
    const before = await sample(), cpu = process.cpuUsage(), started = performance.now(), initialBytes = payloadBytes;
    const initial = await viewer.evaluate(() => view.diagnostics());
    await viewer.waitForTimeout(4000);
    const actionAt = performance.now();
    await viewer.frameLocator('iframe').locator('#action').click();
    await viewer.frameLocator('iframe').locator('#result').filter({ hasText: 'Done' }).waitFor();
    const actionMs = performance.now() - actionAt;
    const after = await sample(), elapsed = performance.now() - started, usage = process.cpuUsage(cpu);
    const final = await viewer.evaluate(() => view.diagnostics());
    assert.ok(final.document.polls > initial.document.polls);
    if (["canvas", "video"].includes(kind)) { assert.ok(final.media.frames > initial.media.frames); assert.ok(final.media.bytes > initial.media.bytes); }
    console.log(JSON.stringify({ kind, elapsedMs: elapsed, payloadBytesPerSecond: (payloadBytes - initialBytes) * 1000 / elapsed, actionMs, captureMs, nodeCpuMs: (usage.user + usage.system) / 1000, hostTaskMs: (after[0].TaskDuration - before[0].TaskDuration) * 1000, viewerTaskMs: (after[1].TaskDuration - before[1].TaskDuration) * 1000, viewerHeapBytes: after[1].JSHeapUsedSize, frames: final.media.frames - initial.media.frames, metrics: final }));
    await viewer.evaluate(() => view.stop()); source.dispose(); await viewer.close(); await host.close();
  }
} finally { await browser?.close(); await new Promise(resolve => server.close(resolve)); }
