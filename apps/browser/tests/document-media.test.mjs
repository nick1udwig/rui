import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { chromium, firefox, webkit } from "playwright";
import { BrowserDocument } from "../document.mjs";

test("scoped WebRTC carries canvas, WebGL, video and audio while text stays DOM", { timeout: 45000 }, async () => {
  const files = new Map(await Promise.all(['document-renderer', 'document-media', 'document-request', 'document-stream'].map(async name => [`/${name}.mjs`, await readFile(new URL(`../src/${name}.mjs`, import.meta.url))])));
  const server = createServer((req, res) => {
    if (files.has(req.url)) { res.setHeader('Content-Type', 'text/javascript'); res.end(files.get(req.url)); }
    else { res.setHeader('Content-Type', 'text/html'); res.end('<h1>Native text</h1>'); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser, receiverBrowser, viewer, source;
  try {
    browser = await chromium.launch({ executablePath: process.env.RUI_CHROMIUM || '/usr/bin/chromium', headless: true, chromiumSandbox: true, args: ['--autoplay-policy=no-user-gesture-required', '--enable-unsafe-swiftshader', ...(process.env.RUI_TEST_DIRECT_ICE ? ['--disable-features=WebRtcHideLocalIpsWithMdns'] : [])] });
    const host = await browser.newPage();
    await host.goto(`http://127.0.0.1:${server.address().port}`);
    await host.evaluate(async () => {
      const canvas = document.createElement('canvas'); canvas.id = 'drawing'; canvas.width = canvas.height = 32; document.body.append(canvas);
      const glCanvas = document.createElement('canvas'); glCanvas.id = 'gl'; glCanvas.width = glCanvas.height = 32; document.body.append(glCanvas);
      const context = canvas.getContext('2d'), gl = glCanvas.getContext('webgl');
      if (!gl) throw Error('WebGL required');
      globalThis.fixtureColor = 'red';
      const draw = () => { context.fillStyle = fixtureColor; context.fillRect(0,0,32,32); gl.clearColor(0,1,0,1); gl.clear(gl.COLOR_BUFFER_BIT); requestAnimationFrame(draw); }; draw();
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
    const engine = process.env.RUI_CLIENT_ENGINE;
    receiverBrowser = engine ? await { chromium, firefox, webkit }[engine].launch({ headless: true, ...(process.env.RUI_CLIENT_EXECUTABLE ? { executablePath: process.env.RUI_CLIENT_EXECUTABLE } : {}) }) : browser;
    viewer = await receiverBrowser.newPage();
    const blocked = process.env.RUI_TEST_BLOCK_ICE === '1';
    if (blocked) await viewer.addInitScript(() => {
      const Peer = RTCPeerConnection;
      window.RTCPeerConnection = class extends Peer { constructor(config) { super({ ...config, iceTransportPolicy: 'relay', iceServers: [] }); } };
    });
    await viewer.goto(`http://127.0.0.1:${server.address().port}`);
    let failedReads = 0, failedToken;
    await viewer.exposeFunction('mediaApi', async (_, request) => {
      if (request.op === 'open') return source.media.open(request.node, request.generation, request.transport, request.types);
      if (request.op === 'close') return source.media.close(request.token);
      if (request.op === 'read') {
        if (failedReads > 0 && request.token === failedToken) { failedReads--; throw Error('Injected media read failure'); }
        return source.media.read(request.token, request.after);
      }
      return source.media.update(request.token, request.answer);
    });
    await viewer.evaluate(async snapshot => {
      const { DocumentRenderer } = await import('/document-renderer.mjs');
      const { DocumentMediaView } = await import('/document-media.mjs');
      globalThis.mediaErrors = [];
      globalThis.media = new DocumentMediaView(globalThis.mediaApi, error => mediaErrors.push(String(error)));
      const iframe = document.createElement('iframe'); document.body.replaceChildren(iframe);
      const spacer = document.createElement('div'); spacer.style.height = '2000px'; document.body.append(spacer);
      const renderer = new DocumentRenderer(iframe, () => {}, () => false, media);
      globalThis.render = async snapshot => { media.begin(); await renderer.render(snapshot); media.finish(); };
      await render(snapshot);
      globalThis.pollMedia = setInterval(() => { media.begin(); for (const node of snapshot.root.children.flatMap(n => n.children || [])) if (node.media) media.attach(node, iframe.contentDocument.getElementById(node.attrs.id), snapshot.generation, []); media.finish(); }, 300);
    }, snapshot);
    await viewer.waitForFunction(() => {
      const doc = document.querySelector('iframe').contentDocument;
      const red = doc.querySelector('#drawing')?.getContext('2d').getImageData(10,10,1,1).data;
      const green = doc.querySelector('#gl')?.getContext('2d').getImageData(10,10,1,1).data;
      return red?.[0] > 200 && green?.[1] > 200 && doc.querySelector('#movie')?.readyState >= 2;
    }, null, { timeout: 15000 });
    for (const [id, channel] of [['drawing', 0], ['gl', 1]]) {
      const box = await viewer.frames()[1].locator(`#${id}`).boundingBox();
      assert.ok(box.width >= 32 && box.height >= 32);
      await viewer.screenshot({ path: new URL('../test-output/media-visible.png', import.meta.url).pathname });
      const png = (await viewer.screenshot({ clip: box })).toString('base64');
      const pixel = await viewer.evaluate(async png => {
        const image = new Image(); image.src = 'data:image/png;base64,' + png; await image.decode();
        const canvas = document.createElement('canvas'); canvas.width = image.width; canvas.height = image.height;
        const context = canvas.getContext('2d'); context.drawImage(image, 0, 0);
        return [...context.getImageData(10, 10, 1, 1).data];
      }, png);
      assert.ok(pixel[channel] > 200 && pixel[(channel + 1) % 3] < 60, `${id} must be visible on screen: ${pixel}`);
    }
    const audio = await viewer.evaluate(async () => {
      const entry = [...media.streams.values()].find(entry => entry.element.id === 'movie');
      if (entry.transport === 'https') return entry.player.captureStream().getAudioTracks().length > 0;
      const stats = await entry.peer.getStats();
      return [...stats.values()].some(s => s.type === 'inbound-rtp' && s.kind === 'audio' && s.packetsReceived > 0);
    });
    assert.equal(audio, true);
    if (blocked) assert.equal(await viewer.evaluate(() => [...media.streams.values()].every(entry => entry.transport === 'https')), true, 'Blocked direct connections fall back through HTTPS');
    else assert.deepEqual(await viewer.evaluate(() => mediaErrors), []);
    await viewer.evaluate(() => {
      const button = document.createElement('button'); button.textContent = 'Enable page sound'; button.onclick = () => media.enableSound(); document.body.prepend(button);
    });
    await viewer.getByRole('button', { name: 'Enable page sound' }).click();
    await viewer.waitForFunction(() => { const video = document.querySelector('iframe').contentDocument.querySelector('#movie'); return !video.paused && !video.muted && video.readyState >= 2; });
    if (blocked) {
      await viewer.evaluate(async () => {
        const audio = new AudioContext(); await audio.resume();
        const source = audio.createMediaElementSource(document.querySelector('iframe').contentDocument.querySelector('#movie'));
        const analyser = globalThis.soundAnalyser = audio.createAnalyser();
        const silent = audio.createGain(); silent.gain.value = 0;
        source.connect(analyser); analyser.connect(silent); silent.connect(audio.destination);
      });
      await viewer.waitForFunction(() => { const samples = new Uint8Array(soundAnalyser.fftSize); soundAnalyser.getByteTimeDomainData(samples); return Math.max(...samples) - Math.min(...samples) > 20; });
      const previous = await viewer.evaluate(() => [...media.streams.values()].find(entry => entry.element.id === 'movie').token);
      failedReads = 1; failedToken = previous;
      await viewer.waitForFunction(token => { const entry = [...media.streams.values()].find(entry => entry.element.id === 'movie'); return entry?.token && entry.token !== token && entry.player.readyState >= 2 && !entry.player.paused; }, previous);
      await assert.rejects(source.media.read(previous, 0), /expired/);
    }
    assert.equal(await viewer.frames()[1].locator('h1').textContent(), 'Native text');
    await host.evaluate(async () => {
      fixtureColor = 'blue';
      const recorder = new MediaRecorder(document.querySelector('#drawing').captureStream(30), { mimeType: 'video/webm;codecs=vp8' });
      const chunks = []; recorder.ondataavailable = event => chunks.push(event.data);
      const done = new Promise(resolve => recorder.onstop = resolve); recorder.start();
      await new Promise(resolve => setTimeout(resolve, 800)); recorder.stop(); await done;
      const video = document.querySelector('#movie');
      video.src = URL.createObjectURL(new Blob(chunks, { type: 'video/webm' }));
      await video.play();
    });
    let replaced = false;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !replaced) {
      await viewer.evaluate(snapshot => render(snapshot), await source.capture());
      replaced = await viewer.evaluate(() => {
        const video = document.querySelector('iframe').contentDocument.querySelector('#movie');
        if (video.readyState < 2) return false;
        const canvas = document.createElement('canvas'); canvas.width = canvas.height = 32;
        const context = canvas.getContext('2d'); context.drawImage(video, 0, 0, 32, 32);
        const pixel = context.getImageData(10, 10, 1, 1).data;
        return pixel[2] > 200 && pixel[0] < 60;
      });
      if (!replaced) await viewer.waitForTimeout(250);
    }
    assert.equal(replaced, true, 'Replacing the host video source delivers new frames, not a frozen frame from the previous clip');
    await viewer.evaluate(() => { clearInterval(pollMedia); media.dispose(); });
    await host.waitForFunction(key => globalThis[key].media.size === 0, source.key);
  } catch (error) {
    if (viewer) console.error(await viewer.evaluate(async () => ({ errors: globalThis.mediaErrors, streams: await Promise.all([...globalThis.media.streams.values()].map(async e => ({ id: e.element.id, transport: e.transport, ready: e.player.readyState, paused: e.player.paused, state: e.peer?.connectionState, stats: e.peer ? [...(await e.peer.getStats()).values()].filter(s => s.type === "inbound-rtp") : undefined }))) })));
    throw error;
  } finally {
    source?.dispose();
    if (receiverBrowser !== browser) await receiverBrowser?.close();
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
});
