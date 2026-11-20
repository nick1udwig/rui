import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('static canvas and paused video deliver first frames and recover after being hidden', { timeout: 45000 }, async () => {
  const modules = new Map(await Promise.all(['document-renderer', 'document-media', 'document-request', 'document-stream'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => { res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html'); res.end(modules.get(req.url) || '<body></body>'); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true, args: ['--autoplay-policy=no-user-gesture-required'] });
  let source;
  try {
    const host = await browser.newPage(), viewer = await browser.newPage();
    const origin = `http://127.0.0.1:${server.address().port}`;
    await host.goto(origin); await viewer.goto(origin);
    await host.evaluate(async () => {
      const canvas = document.createElement('canvas'); canvas.id = 'still'; canvas.width = 320; canvas.height = 180;
      const context = canvas.getContext('2d'); context.fillStyle = 'red'; context.fillRect(0, 0, 320, 180); document.body.append(canvas);
      const recording = document.createElement('canvas'); recording.width = 320; recording.height = 180;
      const ctx = recording.getContext('2d');
      const paint = setInterval(() => { ctx.fillStyle = 'blue'; ctx.fillRect(0, 0, 320, 180); }, 30);
      const stream = recording.captureStream(30), recorder = new MediaRecorder(stream, { mimeType: 'video/webm;codecs=vp8' }), chunks = [];
      recorder.ondataavailable = event => chunks.push(event.data);
      const done = new Promise(resolve => recorder.onstop = resolve); recorder.start(); await new Promise(resolve => setTimeout(resolve, 1000)); recorder.stop(); await done;
      clearInterval(paint); stream.getTracks().forEach(track => track.stop());
      const video = document.createElement('video'); video.id = 'paused'; video.muted = true; video.src = URL.createObjectURL(new Blob(chunks, { type: 'video/webm' })); document.body.append(video);
      await video.play(); video.pause();
    });
    source = new BrowserDocument(host);
    let bytes = 0;
    await viewer.exposeFunction('api', async (_, data) => {
      if (data.op === 'open') return source.media.open(data.node, data.generation, data.transport, data.types, data.quality);
      if (data.op === 'visibility') return source.media.visibility(data.token, data.visible);
      if (data.op === 'close') return source.media.close(data.token);
      const result = await source.media.read(data.token, data.after);
      bytes += result.chunks.reduce((n, chunk) => n + Buffer.from(chunk.data, 'base64').length, 0);
      return result;
    });
    await viewer.evaluate(async snapshot => {
      const { DocumentRenderer } = await import('/document-renderer.mjs'), { DocumentMediaView } = await import('/document-media.mjs');
      const frame = document.createElement('iframe'); frame.style = 'width:700px;height:500px'; document.body.append(frame);
      window.errors = []; window.media = new DocumentMediaView(api, error => errors.push(String(error)));
      const renderer = new DocumentRenderer(frame, () => {}, () => true, media);
      media.begin(); await renderer.render(snapshot); media.finish();
      window.pixel = id => {
        const element = frame.contentDocument.getElementById(id), canvas = document.createElement('canvas'); canvas.width = canvas.height = 1;
        const context = canvas.getContext('2d'); context.drawImage(element, 0, 0, 1, 1); return [...context.getImageData(0, 0, 1, 1).data];
      };
    }, await source.capture());
    await viewer.waitForFunction(() => pixel('still')[0] > 200 && pixel('paused')[2] > 200, null, { timeout: 8000 }).catch(async error => {
      console.log(await viewer.evaluate(() => JSON.stringify({ errors, metrics: media.diagnostics(), still: pixel('still'), paused: pixel('paused'), streams: [...media.streams.values()].map(e => { const c=document.createElement('canvas'); c.width=c.height=1; const ctx=c.getContext('2d'); ctx.drawImage(e.player,0,0,1,1); return { id:e.element.id, frameAt:e.frameAt, width:e.element.width, height:e.element.height, pixel:[...ctx.getImageData(0,0,1,1).data] }; }) })));
      console.log(await host.evaluate(key => [...globalThis[key].media.values()].map(s => ({ chunks: s.sequence, bytes: s.bytes, ready: s.capturePlayer?.readyState, frames: s.capturePlayer?.getVideoPlaybackQuality().totalVideoFrames, state: s.recorder.state })), source.key));
      throw error;
    });
    assert.equal(await host.locator('#paused').evaluate(video => video.paused), true, 'Starting the replica must not start the paused host');
    const before = bytes;
    await viewer.waitForTimeout(2000);
    assert.ok(bytes - before < 20000, 'Still pictures retain a small encoded byte budget');
    await viewer.evaluate(async () => {
      const entry = [...media.streams.values()].find(entry => entry.element.id === 'still');
      entry.element.style.display = 'none'; await media.adapt(entry); clearTimeout(entry.adaptation);
    });
    await host.locator('#still').evaluate(canvas => { const ctx = canvas.getContext('2d'); ctx.fillStyle = 'green'; ctx.fillRect(0, 0, 320, 180); });
    await viewer.waitForTimeout(800);
    await viewer.evaluate(async () => {
      const entry = [...media.streams.values()].find(entry => entry.element.id === 'still');
      entry.element.style.display = ''; await media.adapt(entry); clearTimeout(entry.adaptation);
    });
    await viewer.waitForFunction(() => pixel('still')[1] > 90 && pixel('still')[0] < 30, null, { timeout: 8000 });
    assert.deepEqual(await viewer.evaluate(() => errors), []);
    await viewer.evaluate(() => media.dispose());
  } finally { source?.dispose(); await browser.close(); await new Promise(resolve => server.close(resolve)); }
});
