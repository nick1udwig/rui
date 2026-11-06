import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { mkdtemp, rm, readFile, mkdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join, resolve } from 'node:path';
import assert from 'node:assert/strict';

const directory = await mkdtemp('/tmp/rui-control-');
const root = resolve(import.meta.dirname, '../../..');
const output = resolve(import.meta.dirname, '../test-output');
await mkdir(output, { recursive: true });
const cert = join(directory, 'cert.pem'), key = join(directory, 'key.pem');
execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=localhost', '-keyout', key, '-out', cert], { stdio: 'ignore' });
const fixture = createServer((req, res) => {
  if (req.url === '/oversized') {
    res.setHeader('Content-Type', 'text/html');
    res.end('<title>Page limit fixture</title><svg width="100" height="100">' + '<path d="M0 0L10 10"/>'.repeat(21000) + '</svg>');
    return;
  }
  const send = () => { res.setHeader('Content-Type', 'text/html'); res.end('<title>Control fixture</title><button id=search onclick="document.querySelector(\'input\').hidden=false;document.body.dataset.clicks=Number(document.body.dataset.clicks||0)+1">Search</button><input hidden aria-label="Query"><p id=result>Ready</p><button id=dialog onclick="alert(\'Action received\')">Open dialog</button>'); };
  if (req.url === '/slow') setTimeout(send, 2500); else send();
});
await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
const socket = createServer();
await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
const port = socket.address().port;
await new Promise(resolve => socket.close(resolve));
const until = async (fn, message, ms = 30000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await fn()) return; await new Promise(resolve => setTimeout(resolve, 100)); }
  throw Error(message);
};
let source, browser, viewer, host, logs = '';
try {
  source = await chromium.launchPersistentContext(join(directory, 'source'), { executablePath: '/usr/bin/chromium', headless: process.env.RUI_TEST_HEADED !== '1', chromiumSandbox: true, viewport: null, args: ['--remote-debugging-port=0'] });
  const cdpPort = (await readFile(join(directory, 'source/DevToolsActivePort'), 'utf8')).split('\n')[0];
  const page = source.pages()[0];
  host = spawn('python3', ['apps/browser/run.py', '--skip-build', '--cdp-url', `http://127.0.0.1:${cdpPort}`, '--port', String(port), '--host', 'localhost', '--cert', cert, '--key', key, '--files-dir', directory, '--state-dir', join(directory, 'state')], { cwd: root, stdio: ['pipe', 'pipe', 'pipe'] });
  host.stdout.on('data', d => logs += d);
  host.stderr.on('data', d => { logs += d; if (String(d).includes('Document request failed:')) console.error(String(d).trim()); });
  await until(() => /Open on your phone: (https:\/\/[^\s]+)/.test(logs), 'Host starts');
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  viewer = await browser.newPage({ ignoreHTTPSErrors: true, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
  const clientErrors = [];
  viewer.on('pageerror', error => clientErrors.push(error.message));
  const latency = Number(process.env.RUI_TEST_LATENCY_MS || 0);
  const recovery = process.env.RUI_TEST_RECOVERY === '1';
  if (recovery) await viewer.addInitScript(() => {
    const originalFetch = window.fetch;
    window.fetch = async (...args) => {
      const response = await originalFetch(...args);
      if (response.headers.get('content-type') === 'application/vnd.rui.media') {
        const getReader = response.body.getReader.bind(response.body);
        response.body.getReader = () => {
          const reader = getReader(), read = reader.read.bind(reader);
          reader.read = async () => {
            const chunk = await read();
            if (window.ruiBreakMedia) { window.ruiBreakMedia = false; window.ruiMediaBreaks = (window.ruiMediaBreaks || 0) + 1; throw Error('Injected media stream failure'); }
            return chunk;
          };
          return reader;
        };
      }
      return response;
    };
  });
  const blockedIce = process.env.RUI_TEST_BLOCK_ICE === '1';
  if (blockedIce) await viewer.addInitScript(() => {
    const Peer = RTCPeerConnection;
    window.RTCPeerConnection = class extends Peer { constructor(config) { super({ ...config, iceTransportPolicy: 'relay', iceServers: [] }); } };
  });
  const faults = { document: 0, open: 0, renew: 0, read: 0 }, injected = { document: 0, open: 0, renew: 0, read: 0 };
  let mediaOpens = 0, mediaReads = 0, peerMediaOpens = 0;
  viewer.on('request', request => {
    if (!request.url().endsWith('/api/media')) return;
    const data = request.postDataJSON();
    if (['read', 'stream'].includes(data.op)) mediaReads++;
    if (data.op === 'open' && data.transport !== 'https') peerMediaOpens++;
  });
  if (latency || recovery || blockedIce) await viewer.route('**/api/**', async route => {
    if (latency) await new Promise(resolve => setTimeout(resolve, latency));
    const path = new URL(route.request().url()).pathname;
    const kind = path.endsWith('/document') ? 'document' : path.endsWith('/media') ? route.request().postDataJSON().op : '';
    if (kind === 'open') mediaOpens++;
    if (faults[kind] > 0) {
      faults[kind]--; injected[kind]++;
      await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'Injected temporary failure' }) });
      return;
    }
    if (!viewer.isClosed()) await route.continue();
  });
  const transfer = { decoded: 0, encoded: 0, compressed: 0, resourceRequests: 0 };
  const transfers = [];
  viewer.on('response', response => {
    if (!/\/api\/document(?:-resource)?$/.test(response.url())) return;
    transfers.push((async () => {
      if (response.url().endsWith('/api/document-resource')) transfer.resourceRequests++;
      const body = await response.body();
      const sizes = await response.request().sizes();
      transfer.decoded += body.length;
      transfer.encoded += sizes.responseBodySize;
      if (response.headers()['content-encoding'] === 'gzip') transfer.compressed++;
    })().catch(() => {}));
  });
  if (process.env.RUI_DEBUG_DOCUMENT === '1') {
    const started = new WeakMap();
    viewer.on('request', request => started.set(request, performance.now()));
    viewer.on('response', async response => {
      if (!response.url().endsWith('/api/document')) return;
      const packet = await response.json().catch(() => ({}));
      console.log('Document response', { ms: Math.round(performance.now() - started.get(response.request())), nodes: packet.put?.length, bytes: JSON.stringify(packet).length, error: packet.error });
    });
  }
  await viewer.routeWebSocket('**/rui', socket => {
    const server = socket.connectToServer();
    let closed = false;
    socket.onClose(() => { closed = true; server.close(); });
    server.onClose(() => { closed = true; socket.close(); });
    server.onMessage(message => setTimeout(() => { if (!closed) socket.send(message); }, 250));
  });
  await viewer.goto(/Open on your phone: (https:\/\/[^\s]+)/.exec(logs)[1]);
  await until(() => /Approve locally: approve ([a-f0-9]+)/.test(logs), 'Pairing request');
  host.stdin.write(`approve ${/Approve locally: approve ([a-f0-9]+)/.exec(logs)[1]}\n`);
  await viewer.locator('#control').click();
  await until(async () => await viewer.locator('#connection').textContent() === 'Controlling host', 'Control acquired');
  const address = viewer.getByLabel('Address', { exact: true });
  const go = viewer.getByRole('button', { name: 'Go', exact: true });
  const navigate = async url => { await address.fill(url); await go.click(); };
  const replica = viewer.frameLocator('iframe[title="Host web page"]');
  await until(async () => (await page.evaluate(() => innerWidth)) === await viewer.locator('iframe[title="Host web page"]').evaluate(frame => Math.round(frame.clientWidth)), 'Initial viewport synchronized');
  if (process.env.RUI_TEST_PUBLIC === '1') {
    await navigate('youtube.com');
    const search = replica.getByRole('button', { name: 'Search', exact: true }).first();
    await search.waitFor({ state: 'visible', timeout: 30000 });
    await viewer.evaluate(() => scrollTo(0, 0));
    await search.tap();
    await until(async () => await page.locator('input[name="search_query"]').isVisible(), 'YouTube search opens on host');
    const query = replica.locator('input[name="search_query"]');
    const queryText = 'Big Buck Bunny Blender official';
    await query.fill(queryText);
    const searchStarted = performance.now();
    await query.press('Enter');
    await until(() => new URL(page.url()).pathname === '/results' && new URL(page.url()).searchParams.get('search_query') === queryText, 'YouTube query submitted on host');
    await until(async () => (await address.inputValue()).includes('/results?search_query='), 'YouTube navigation published');
    let result = replica.locator('ytd-video-renderer').first();
    await result.locator('#video-title').waitFor({ state: 'visible', timeout: 30000 });
    await until(async () => result.locator('img').evaluateAll(images => images.some(image => image.complete && image.naturalWidth > 0)), 'Search result thumbnail is rendered');
    const firstTitle = (await result.locator('#video-title').textContent()).trim();
    const resultsScrolls = Number(process.env.RUI_TEST_RESULTS_SCROLLS || 0);
    for (let index = 0; resultsScrolls && index < Math.max(resultsScrolls, 12); index++) {
      if (index >= resultsScrolls && await replica.locator('[data-rui-deferred]').count()) break;
      const before = await page.locator('ytd-video-renderer').count();
      await until(async () => {
        if ((await page.locator('ytd-video-renderer').count()) > before) return true;
        await replica.locator('html').evaluate(node => node.scrollTo(0, node.scrollHeight));
        return false;
      }, 'Phone scrolling loads more host results');
      console.log(JSON.stringify({ resultsBatch: index + 1, hostResultCount: await page.locator('ytd-video-renderer').count() }));
    }
    if (resultsScrolls) {
      await until(async () => await replica.locator('[data-rui-deferred]').count() > 0, 'Large results page uses deferred semantic sections');
      console.log(JSON.stringify({ resultsScrolls, deferredSections: await replica.locator('[data-rui-deferred]').count(), hostResultCount: await page.locator('ytd-video-renderer').count() }));
      await replica.locator('html').evaluate(node => node.scrollTo(0, 0));
      await until(async () => await page.evaluate(() => scrollY < 1), 'Phone returns to the top of the host results');
      result = replica.locator('ytd-video-renderer').filter({ has: replica.locator('#video-title').filter({ hasText: firstTitle }) }).first();
      await result.locator('#video-title').waitFor({ state: 'visible', timeout: 30000 });
    }
    const searchToRenderedMs = Math.round(performance.now() - searchStarted);
    await viewer.screenshot({ path: join(output, 'youtube-results.png') });
    const videoStarted = performance.now();
    if (recovery) { faults.document = 2; faults.open = 2; }
    const videoTarget = process.env.RUI_TEST_VIDEO_TARGET === 'thumbnail' ? 'a#thumbnail' : '#video-title';
    await result.locator(videoTarget).tap();
    await until(() => new URL(page.url()).pathname === '/watch', 'Result opens video on host');
    const player = replica.locator('video').first();
    await player.waitFor({ state: 'visible', timeout: 30000 });
    if (await page.locator('video').first().evaluate(video => video.paused)) {
      const play = replica.locator('.ytp-large-play-button');
      if (await play.isVisible()) await play.tap(); else await player.tap();
    }
    await until(async () => player.evaluate(video => video.readyState >= 2 && video.videoWidth > 0), 'Video frames reach mobile replica');
    const before = await player.evaluate(video => video.currentTime);
    await until(async () => (await player.evaluate(video => video.currentTime)) > before + .5, 'Mobile video advances');
    const videoToFramesMs = Math.round(performance.now() - videoStarted);
    await until(async () => !(await page.locator('#movie_player').evaluate(player => player.classList.contains('ad-showing'))), 'YouTube pre-roll finishes', 120000);
    const contentTime = await page.locator('video').first().evaluate(video => video.currentTime);
    await until(async () => (await page.locator('video').first().evaluate(video => video.currentTime)) > contentTime + 1, 'Selected video plays on the host');
    await until(async () => player.evaluate(video => !video.paused && video.readyState >= 2 && video.videoWidth > 0), 'Selected video reaches the phone after pre-roll');
    await player.evaluate(video => new Promise((resolve, reject) => {
      let frames = 0, callback;
      const timeout = setTimeout(() => { video.cancelVideoFrameCallback(callback); reject(Error('No new phone video frames after pre-roll')); }, 15000);
      const frame = () => {
        if (++frames >= 3) { clearTimeout(timeout); resolve(); }
        else callback = video.requestVideoFrameCallback(frame);
      };
      callback = video.requestVideoFrameCallback(frame);
    }));
    await viewer.getByRole('button', { name: 'Enable page sound', exact: true }).click();
    await until(async () => player.evaluate(video => !video.paused && !video.muted && video.readyState >= 2), 'Sound activation retains playing video');
    if (process.env.RUI_TEST_PAUSE === '1') {
      const tapVideo = async () => {
        await player.scrollIntoViewIfNeeded();
        const box = await player.boundingBox();
        await viewer.touchscreen.tap(box.x + box.width / 2, box.y + box.height / 2);
      };
      for (let index = 0; index < 3; index++) {
        await tapVideo();
        await until(async () => page.locator('video').first().evaluate(video => video.paused), 'Phone tap pauses the host video');
        await viewer.waitForTimeout([250, 900, 2500][index]);
        await tapVideo();
        await until(async () => page.locator('video').first().evaluate(video => !video.paused), 'Phone tap resumes the host video');
        await player.evaluate(video => new Promise((resolve, reject) => {
          let count = 0, callback;
          const timer = setTimeout(() => { video.cancelVideoFrameCallback(callback); reject(Error('Video frames did not resume on the phone')); }, 15000);
          const frame = () => { if (++count === 3) { clearTimeout(timer); resolve(); } else callback = video.requestVideoFrameCallback(frame); };
          callback = video.requestVideoFrameCallback(frame);
        }));
        console.log(JSON.stringify({ pauseResume: index + 1 }));
      }
    }
    assert.ok(mediaReads > 0, 'Video arrives over the page connection');
    assert.equal(peerMediaOpens, 0, 'The default media path does not negotiate a separate peer connection');
    await viewer.screenshot({ path: join(output, 'youtube-video.png') });
    await Promise.all(transfers);
    assert.ok(transfer.compressed > 0 && transfer.encoded < transfer.decoded, 'Document and resource traffic is compressed');
    console.log(JSON.stringify({ youtubeResultsAndVideo: true, contentAfterPreroll: true, soundEnabled: true, blockedIce, mediaReads, latencyMs: latency, searchToRenderedMs, videoToFramesMs, ...transfer }));
    if (recovery) {
      assert.equal(injected.open, 2, 'Media opens fail twice before recovery');
      await player.evaluate(video => {
        video.pause();
        const play = video.play;
        video.play = function () {
          this.play = play;
          window.parent.ruiPlayAborts = (window.parent.ruiPlayAborts || 0) + 1;
          return Promise.reject(new DOMException('The play() request was interrupted by a new load request', 'AbortError'));
        };
        window.parent.ruiSavedVideo = video;
      });
      await viewer.getByRole('button', { name: 'Enable page sound', exact: true }).click();
      await until(async () => player.evaluate(video => !video.paused && video.readyState >= 2), 'Playback resumes after an interrupted play call');
      assert.equal(await viewer.evaluate(() => ruiPlayAborts), 1);
      const opensBeforeDocumentErrors = mediaOpens;
      faults.document = 2;
      await until(() => injected.document === 4, 'Document failures injected during playback');
      await until(async () => viewer.locator('iframe[title="Host web page"]').evaluate(frame => !frame.inert), 'Document polling recovers');
      assert.equal(await player.evaluate(video => video === window.parent.ruiSavedVideo), true, 'Document recovery retains the playing element');
      assert.equal(mediaOpens, opensBeforeDocumentErrors, 'Document errors do not reconnect healthy media');
      await viewer.evaluate(() => { window.ruiBreakMedia = true; });
      await until(async () => await viewer.evaluate(() => window.ruiMediaBreaks === 1) && mediaOpens > opensBeforeDocumentErrors, 'Failed media stream reconnects');
      await until(async () => player.evaluate(video => !video.paused && video.readyState >= 2), 'Replacement stream starts');
      const recoveredTime = await player.evaluate(video => video.currentTime);
      await until(async () => (await player.evaluate(video => video.currentTime)) > recoveredTime + .5, 'Replacement stream advances');
      console.log(JSON.stringify({ recovery: true, injected, interruptedPlayRecovered: true, mediaOpens }));
      await page.evaluate(() => {
        const end = performance.now() + 7000;
        while (performance.now() < end) {}
      });
      assert.equal(await viewer.locator('#connection').textContent(), 'Controlling host', 'A slow source page cannot stall the control connection past its RPC deadline');
      console.log('Control connection survived a seven-second host-page stall');
    }
  }
  await navigate(`http://127.0.0.1:${fixture.address().port}/oversized`);
  await viewer.getByText('This page exceeds the current page-view limit.', { exact: false }).last().waitFor();
  await until(async () => (await address.inputValue()).endsWith('/oversized'), 'Capture-limit navigation is published to the address bar');
  await until(async () => !(await go.isDisabled()), 'Page capture limits leave navigation available');
  await navigate(`http://127.0.0.1:${fixture.address().port}/slow`);
  await replica.locator('#search').waitFor();
  await until(async () => !(await go.isDisabled()), 'Slow navigation must not wedge controls');
  await replica.locator('#search').tap();
  await until(async () => await page.getByLabel('Query').isVisible(), 'Tap reaches host');
  await until(async () => !(await viewer.locator('#retry').isVisible()), 'Tap acknowledged');
  await address.fill('https://example.com/draft');
  await address.evaluate(input => {
    window.readonlyDuringDraft = false;
    new MutationObserver(() => { if (input.readOnly) window.readonlyDuringDraft = true; }).observe(input, { attributes: true, attributeFilter: ['readonly'] });
  });
  await viewer.setViewportSize({ width: 390, height: 520 });
  await until(async () => await page.evaluate(() => innerHeight) === Math.round(520 * .75), 'Keyboard-sized viewport reaches host');
  await until(async () => !(await go.isDisabled()), 'Resize acknowledged');
  assert.equal(await address.inputValue(), 'https://example.com/draft');
  assert.equal(await viewer.evaluate(() => window.readonlyDuringDraft), false, 'Background commands must leave the address draft editable');
  assert.equal(await address.evaluate(input => input === document.activeElement), true, 'Background resize retains focus');
  let dialogs = 0;
  page.on('dialog', () => dialogs++);
  const dialogPromise = page.waitForEvent('dialog');
  await replica.locator('#dialog').tap();
  const dialog = await dialogPromise;
  await until(async () => (await viewer.locator('#retry').textContent()).includes('Check host'), 'Uncertain action offers reconnect');
  await dialog.dismiss();
  await viewer.locator('#retry').click();
  await viewer.locator('#control').click();
  await until(async () => await viewer.locator('#connection').textContent() === 'Controlling host', 'Reconnect recovers control');
  await replica.locator('#search').tap();
  await until(async () => await page.locator('body').getAttribute('data-clicks') === '2', 'Commands work after recovery');
  assert.equal(dialogs, 1, 'Recovery never replays the uncertain action');
  assert.deepEqual(clientErrors, [], 'Recovery leaves no unhandled client errors');
  console.log('Paired control passed: capture-limit recovery, slow navigation, touch search, keyboard resize and uncertain-action recovery');
} catch (error) {
  console.error('Host diagnostics:', logs.split('\n').filter(line => !line.includes('\u001b')).join('\n').replace(/invite=[a-f0-9]+/g, 'invite=[REDACTED]'));
  if (viewer) { console.error('Geometry:', await viewer.evaluate(() => {const frame=document.querySelector('iframe');const doc=frame?.contentDocument; const target=doc?.querySelector('button[aria-label=Search]');return {scroll:scrollY,frame:frame?.getBoundingClientRect().toJSON(),target:target?.getBoundingClientRect().toJSON(),width:doc?.defaultView.innerWidth,height:doc?.defaultView.innerHeight,hidden:frame?.hidden}})); console.error('Client status:', await viewer.locator('#status').textContent(), await viewer.locator('#view').innerText()); console.error('Source:', source.pages()[0].url(), await source.pages()[0].locator('button').evaluateAll(nodes => nodes.map(n => ({label:n.getAttribute('aria-label'),text:n.textContent,id:n.id,visible:!!n.getClientRects().length}))).catch(()=>[])); await viewer.screenshot({path:join(output, 'control-failure.png')}); }
  throw error;
} finally {
  await browser?.close();
  if (host && host.exitCode === null) {
    host.stdin.write('quit\n');
    await Promise.race([new Promise(resolve => host.once('exit', resolve)), new Promise(resolve => setTimeout(resolve, 8000))]);
    if (host.exitCode === null) host.kill('SIGTERM');
  }
  await source?.close();
  await new Promise(resolve => fixture.close(resolve));
  await rm(directory, { recursive: true, force: true });
}
