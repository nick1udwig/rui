import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { chromium } from 'playwright';
import { BrowserDocument } from '../document.mjs';

test('a blocked phone player starts locally before taps control host pause and resume', { timeout: 45000 }, async () => {
  const modules = new Map(await Promise.all(['document-renderer', 'document-media', 'document-request', 'document-stream'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => { res.setHeader('Content-Type', modules.has(req.url) ? 'text/javascript' : 'text/html'); res.end(modules.get(req.url) || '<h1>Media control</h1>'); });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser, source;
  try {
    browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true, args: ['--autoplay-policy=no-user-gesture-required'] });
    const host = await browser.newPage();
    await host.goto(origin);
    await host.evaluate(async () => {
      const canvas = document.createElement('canvas'); canvas.width = 320; canvas.height = 180;
      const context = canvas.getContext('2d');
      const paint = () => { context.fillStyle = `rgb(${Math.floor(performance.now() / 10) % 255},40,180)`; context.fillRect(0, 0, 320, 180); requestAnimationFrame(paint); }; paint();
      const recorder = new MediaRecorder(canvas.captureStream(30), { mimeType: 'video/webm;codecs=vp8' });
      const chunks = []; recorder.ondataavailable = event => chunks.push(event.data);
      const done = new Promise(resolve => recorder.onstop = resolve); recorder.start();
      await new Promise(resolve => setTimeout(resolve, 1200)); recorder.stop(); await done;
      const video = document.createElement('video'); video.id = 'movie'; video.loop = true; video.muted = true;
      video.src = URL.createObjectURL(new Blob(chunks, { type: 'video/webm' }));
      video.onclick = () => { if (video.paused) void video.play(); else video.pause(); };
      document.body.append(video); await video.play();
    });
    source = new BrowserDocument(host);
    const viewer = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
    await viewer.goto(origin);
    const commands = [];
    await viewer.exposeFunction('dispatch', async command => { commands.push(command); await source.dispatch(command); });
    await viewer.exposeFunction('mediaApi', async (_, request) => {
      if (request.op === 'open') return source.media.open(request.node, request.generation, request.transport, request.types);
      if (request.op === 'read') return source.media.read(request.token, request.after);
      if (request.op === 'close') return source.media.close(request.token);
      return source.media.update(request.token, request.answer);
    });
    await viewer.evaluate(async () => {
      const { DocumentRenderer } = await import('/document-renderer.mjs');
      const { DocumentMediaView } = await import('/document-media.mjs');
      window.errors = [];
      window.media = new DocumentMediaView(mediaApi, error => errors.push(String(error)));
      media.preferHttps = true;
      const frame = document.createElement('iframe'); frame.style = 'width:358px;height:633px'; document.body.replaceChildren(frame);
      const renderer = new DocumentRenderer(frame, command => dispatch(command).catch(error => errors.push(String(error))), () => true, media);
      window.render = async snapshot => { media.begin(); await renderer.render(snapshot); media.finish(); };
    });
    const render = async () => viewer.evaluate(snapshot => render(snapshot), await source.capture());
    await render();
    const replica = viewer.frameLocator('iframe').locator('#movie');
    const nextFrame = () => replica.evaluate(video => new Promise((resolve, reject) => {
      const timer = setTimeout(() => { video.cancelVideoFrameCallback(callback); reject(Error('Phone video frame missing')); }, 10000);
      const callback = video.requestVideoFrameCallback(() => { clearTimeout(timer); resolve(); });
    }));
    await nextFrame();
    await viewer.evaluate(() => {
      const entry = [...media.streams.values()][0], play = entry.player.play.bind(entry.player);
      entry.player.pause();
      let gesture = false;
      entry.player.addEventListener('pointerdown', event => { gesture = event.isTrusted; }, { once: true });
      entry.player.play = () => gesture ? play() : Promise.reject(new DOMException('User gesture required', 'NotAllowedError'));
      media.play(entry);
    });
    await viewer.waitForFunction(() => [...media.streams.values()][0].blocked);
    await replica.tap();
    await viewer.waitForTimeout(300);
    assert.equal(commands.length, 0, 'Starting blocked local playback must not send a pause to the host');
    assert.equal(await host.locator('#movie').evaluate(video => video.paused), false);
    await nextFrame();
    await replica.tap();
    await host.waitForFunction(() => document.querySelector('#movie').paused);
    await render();
    await replica.tap();
    await host.waitForFunction(() => !document.querySelector('#movie').paused);
    await render();
    await nextFrame();
    assert.equal(commands.length, 2, 'Pause and resume each reach the host once');
    await viewer.evaluate(() => media.dispose());
  } finally {
    source?.dispose(); await browser?.close(); await new Promise(resolve => server.close(resolve));
  }
});
