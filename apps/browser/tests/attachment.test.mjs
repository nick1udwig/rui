import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from 'playwright';
import { Workspace } from '../workspace.mjs';

test('attachment leaves external browser and excess tabs alive on detach', { timeout: 30000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'rui-attach-'));
  const child = spawn(process.env.RUI_CHROMIUM || '/usr/bin/chromium', ['--headless', '--remote-debugging-port=0', `--user-data-dir=${directory}`, '--no-first-run', 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] });
  let browser, workspace;
  try {
    const endpoint = await new Promise((resolve, reject) => {
      let logs = '';
      const timer = setTimeout(() => reject(Error('CDP startup deadline')), 10000);
      child.once('error', error => { clearTimeout(timer); reject(error); });
      child.stderr.on('data', chunk => { logs += chunk; const match = /DevTools listening on (ws:\/\/\S+)/.exec(logs); if (match) { clearTimeout(timer); resolve(match[1]); } });
    });
    browser = await chromium.connectOverCDP(endpoint, { noDefaults: true });
    const context = browser.contexts()[0];
    for (let i = context.pages().length; i < 17; i++) await context.newPage();
    process.env.RUI_CDP_URL = endpoint;
    process.env.RUI_FILES_DIR = directory;
    workspace = await Workspace.launch(join(directory, 'rui'));
    assert.equal(workspace.pages.size, 16);
    assert.equal(context.pages().length, 17);
    const doc = workspace.pages.get(workspace.active).document;
    const snapshot = await doc.capture();
    await doc.dispatch({ generation: snapshot.generation, revision: snapshot.revision, action: 'resize', width: 390, height: 600 });
    await workspace.close(); workspace = null;
    assert.equal(child.exitCode, null);
    assert.equal(context.pages().length, 17);
    assert.equal(await context.pages()[0].evaluate(() => 2 + 2), 4);
  } finally {
    delete process.env.RUI_CDP_URL;
    await workspace?.close();
    await browser?.close();
    child.kill('SIGTERM');
    if (child.exitCode === null) await new Promise(resolve => child.once('exit', resolve));
    await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});
