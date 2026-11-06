import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { chromium } from "playwright";
import { BrowserDocument } from "../document.mjs";

test("scoped WebRTC carries canvas, WebGL, video and audio while text stays DOM", { timeout: 45000 }, async () => {
  const files = new Map(await Promise.all(['document-renderer', 'document-media'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => {
    if (files.has(req.url)) { res.setHeader('Content-Type', 'text/javascript'); res.end(files.get(req.url)); }
    else { res.setHeader('Content-Type', 'text/html'); res.end('<h1>Native text</h1>'); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, source;
  try {
    browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || '/usr/bin/chromium', headless: true, chromiumSandbox: true, args: ['--autoplay-policy=no-user-gesture-required', '--enable-unsafe-swiftshader'] });
    const host = await browser.newPage();
    await host.goto(`http://127.0.0.1:${server.address().port}`);
    await host.evaluate(async () => {
      const canvas = document.createElement('canvas'); canvas.id = 'drawing'; canvas.width = canvas.height = 32; document.body.append(canvas);
      const glCanvas = document.createElement('canvas'); glCanvas.id = 'gl'; glCanvas.width = glCanvas.height = 32; document.body.append(glCanvas);
      const context = canvas.getContext('2d'), gl = glCanvas.getContext('webgl');
      if (!gl) throw Error('WebGL required');
      const draw = () => { context.fillStyle = 'red'; context.fillRect(0,0,32,32); gl.clearColor(0,1,0,1); gl.clear(gl.COLOR_BUFFER_BIT); requestAnimationFrame(draw); }; draw();
      const audio = new AudioContext(); await audio.resume();
      const oscillator = audio.createOscillator(), destination = audio.createMediaStreamDestination();
      oscillator.connect(destination); oscillator.start();
      const stream = canvas.captureStream(30); stream.addTrack(destination.stream.getAudioTracks()[0]);
      const recorder = new MediaRecorder(stream, { mimeType: 'video/webm;codecs=vp8,opus' });
      const chunks = []; recorder.ondataavailable = event => chunks.push(event.data);
      const done = new Promise(resolve => recorder.onstop = resolve); recorder.start();
      await new Promise(resolve => setTimeout(resolve, 800)); recorder.stop(); await done;
      const video = document.createElement('video'); video.id = 'movie'; video.loop = true; video.src = URL.createObjectURL(new Blob(chunks, { type: 'video/webm' })); document.body.append(video); await video.play();
      globalThis.fixtureAudio = audio;
    });
    source = new BrowserDocument(host);
    const snapshot = await source.capture();
    const viewer = await browser.newPage();
    await viewer.goto(`http://127.0.0.1:${server.address().port}`);
    await viewer.exposeFunction('mediaApi', async (_, request) => {
      if (request.op === 'open') return source.media.open(request.node, request.generation);
      if (request.op === 'close') return source.media.close(request.token);
      return source.media.update(request.token, request.answer);
    });
    await viewer.evaluate(async snapshot => {
      const { DocumentRenderer } = await import('/document-renderer.mjs');
      const { DocumentMediaView } = await import('/document-media.mjs');
      globalThis.mediaErrors = [];
      globalThis.media = new DocumentMediaView(globalThis.mediaApi, error => mediaErrors.push(String(error)));
      const iframe = document.createElement('iframe'); document.body.replaceChildren(iframe);
      const renderer = new DocumentRenderer(iframe, () => {}, () => false, media);
      media.begin(); await renderer.render(snapshot); media.finish(); media.enableSound();
    }, snapshot);
    await viewer.waitForFunction(() => {
      const doc = document.querySelector('iframe').contentDocument;
      const red = doc.querySelector('#drawing')?.getContext('2d').getImageData(10,10,1,1).data;
      const green = doc.querySelector('#gl')?.getContext('2d').getImageData(10,10,1,1).data;
      return red?.[0] > 200 && green?.[1] > 200 && doc.querySelector('#movie')?.readyState >= 2;
    }, null, { timeout: 15000 });
    const audio = await viewer.evaluate(async () => {
      const entry = [...media.streams.values()].find(entry => entry.element.id === 'movie');
      const stats = await entry.peer.getStats();
      return [...stats.values()].some(s => s.type === 'inbound-rtp' && s.kind === 'audio' && s.packetsReceived > 0);
    });
    assert.equal(audio, true);
    assert.deepEqual(await viewer.evaluate(() => mediaErrors), []);
    assert.equal(await viewer.frames()[1].locator('h1').textContent(), 'Native text');
    await viewer.evaluate(() => media.dispose());
    await host.waitForFunction(key => globalThis[key].media.size === 0, source.key);
  } finally {
    source?.dispose();
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
