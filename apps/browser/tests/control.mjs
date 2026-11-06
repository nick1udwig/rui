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
  host.stderr.on('data', d => logs += d);
  await until(() => /Open on your phone: (https:\/\/[^\s]+)/.test(logs), 'Host starts');
  browser = await chromium.launch({ executablePath: '/usr/bin/chromium', headless: true, chromiumSandbox: true });
  viewer = await browser.newPage({ ignoreHTTPSErrors: true, viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });
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
  if (process.env.RUI_TEST_PUBLIC === '1') {
    await navigate('https://www.youtube.com/');
    const search = replica.getByRole('button', { name: 'Search', exact: true }).first();
    await search.waitFor({ state: 'visible', timeout: 30000 });
    await search.tap();
    await until(async () => await page.locator('input[name="search_query"]').isVisible(), 'YouTube search opens on host');
    const query = replica.locator('input[name="search_query"]');
    await query.fill('browser remote control');
    await query.press('Enter');
    await until(() => page.url().includes('/results?search_query=browser+remote+control'), 'YouTube query submitted on host');
    await until(async () => (await address.inputValue()).includes('/results?search_query='), 'YouTube navigation published');
    console.log('YouTube search opened and query submitted');
  }
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
  console.log('Paired control passed: slow navigation, touch search, keyboard resize and uncertain-action recovery');
} catch (error) {
  if (viewer) { console.error('Client status:', await viewer.locator('#status').textContent(), await viewer.locator('#view').innerText()); console.error('Source:', source.pages()[0].url(), await source.pages()[0].locator('button').evaluateAll(nodes => nodes.map(n => ({label:n.getAttribute('aria-label'),text:n.textContent,id:n.id,visible:!!n.getClientRects().length}))).catch(()=>[])); await viewer.screenshot({path:join(output, 'control-failure.png')}); }
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
