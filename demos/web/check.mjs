import assert from "node:assert/strict";
import { mkdtemp, readFile, readdir, rm, mkdir } from "node:fs/promises";
import { spawn, execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { chromium } from "../../client-browser/node_modules/playwright/index.mjs";
const root = resolve(import.meta.dirname, "../..");
assert.equal(process.env.RUI_ISOLATED_DISPLAY, "1", "private display required");
const temporary = await mkdtemp(resolve(tmpdir(), "rui-browser-check-")),
  identity = resolve(temporary, "identity");
const headed = process.env.RUI_BROWSER_HEADED === "1",
  secure = process.env.RUI_BROWSER_HTTPS === "1",
  built = process.env.RUI_BROWSER_BUILD === "1";
const origin = `${secure ? "https" : "http"}://127.0.0.1:5173`;
const processes = [];
let logs = "",
  browser;
function start(command, args, env = {}) {
  const p = spawn(command, args, {
    cwd: root,
    env: { ...process.env, ...env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  processes.push(p);
  p.stdout.on("data", (b) => (logs += b));
  p.stderr.on("data", (b) => (logs += b));
  return p;
}
async function wait(read, condition = Boolean) {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      const value = await read();
      if (condition(value)) return value;
    } catch {}
    await new Promise((done) => setTimeout(done, 50));
  }
  throw new Error("state did not converge\n" + logs.slice(-4000));
}
async function newest() {
  const files = (await readdir(resolve(identity, "source")))
    .filter((n) => /^whole-\d+\.json$/.test(n))
    .sort();
  return JSON.parse(
    await readFile(resolve(identity, "source", files.at(-1)), "utf8"),
  );
}
function labeled(frame, label) {
  return frame.semantics.some((n) => n.label === label);
}
function hasText(frame, value) {
  return frame.semantics.some((n) => n.text?.content === value);
}
function target(frame, label) {
  const n = frame.semantics.find((n) => n.label === label),
    surface = frame.surfaces.find((s) => s.surface_key === "main-content");
  const [x, y, w, h] = surface.binding_regions.find(
    (b) => b.semantic_id === n.id,
  ).interaction_region;
  return { x: x + w / 2, y: y + h / 2 };
}
function native(operation, ...args) {
  execFileSync(
    "python3",
    [
      resolve(root, "fixtures/render/native_events.py"),
      "RUI Host",
      operation,
      ...args.map(String),
    ],
    { env: process.env, timeout: 5000 },
  );
}
try {
  const host = start(resolve(root, "target/release/rui-demo"), [
    "--host-only",
    "--listen",
    "0.0.0.0:0",
    "--identity-dir",
    identity,
    "--browser-origin",
    origin,
  ]);
  let frame = await wait(newest);
  await wait(
    () => logs,
    (s) => s.includes("Visible GTK host listening at"),
  );
  const port = /Visible GTK host listening at 0.0.0.0:(\d+)/.exec(logs)[1];
  const tlsEnv = {},
    browserArgs = ["--ozone-platform=x11"];
  if (secure) {
    const cert = resolve(temporary, "web.pem"),
      key = resolve(temporary, "web-key.pem");
    execFileSync(
      "openssl",
      [
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "1",
        "-subj",
        "/CN=localhost",
        "-addext",
        "subjectAltName=DNS:localhost,IP:127.0.0.1",
        "-keyout",
        key,
        "-out",
        cert,
      ],
      { stdio: ["ignore", "ignore", "ignore"] },
    );
    const publicKey = execFileSync("openssl", [
      "x509",
      "-in",
      cert,
      "-pubkey",
      "-noout",
    ]);
    const der = execFileSync("openssl", ["pkey", "-pubin", "-outform", "DER"], {
      input: publicKey,
    });
    // Scope the fixture trust exception to this exact owned certificate's key.
    browserArgs.push(
      "--ignore-certificate-errors-spki-list=" +
        createHash("sha256").update(der).digest("base64"),
    );
    Object.assign(tlsEnv, { RUI_WEB_CERT: cert, RUI_WEB_KEY: key });
  }
  start("npm", ["--prefix", "demos/web", "run", built ? "preview" : "dev"], {
    RUI_DEMO_IDENTITY: identity,
    RUI_HOST_URL: `wss://127.0.0.1:${port}`,
    ...tlsEnv,
  });
  await wait(
    () => logs,
    (s) => s.includes("Local:") || s.includes("127.0.0.1:5173"),
  );
  let context, page;
  if (headed) {
    // A second CDP session cannot undo Playwright's focus emulation in its
    // original session. Attach without those overrides to a fresh owned profile
    // instead, so switching tabs exercises Chromium's real visibility events.
    start(process.env.RUI_WASM_CHROMIUM ?? chromium.executablePath(), [
      ...browserArgs,
      "--remote-debugging-address=127.0.0.1",
      "--remote-debugging-port=0",
      `--user-data-dir=${resolve(temporary, "browser")}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-networking",
      "--disable-component-update",
      "--disable-extensions",
      "about:blank",
    ]);
    const endpoint = await wait(
      () =>
        /DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(logs)?.[1],
    );
    browser = await chromium.connectOverCDP(endpoint, { noDefaults: true });
    context = browser.contexts()[0];
    page = context.pages()[0];
    await page.setViewportSize({ width: 1200, height: 1200 });
  } else {
    browser = await chromium.launch({
      headless: true,
      executablePath: process.env.RUI_WASM_CHROMIUM,
      args: browserArgs,
    });
    context = await browser.newContext({
      viewport: { width: 1200, height: 1200 },
    });
    page = await context.newPage();
  }
  page.setDefaultTimeout(12000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(origin + "/");
  await page
    .locator("#credential")
    .setInputFiles(resolve(identity, "client.credential"));
  await page.locator("#connect").click();
  await page
    .locator("#revision")
    .filter({ hasText: /Revision/ })
    .waitFor();
  assert.equal(await page.locator("#connection").textContent(), "Observer");
  const observed = await page.locator("#revision").textContent();
  const b = target(frame, "Increment host counter");
  await page
    .locator("#mirror")
    .click({ position: { x: b.x * 0.75, y: b.y * 0.75 } });
  await new Promise((done) => setTimeout(done, 200));
  assert(
    labeled(await newest(), "Host state: 0 clicks"),
    "observer mutated host",
  );
  await page.locator("#control").click();
  await page.locator("#connection").filter({ hasText: "Controller" }).waitFor();
  const started = performance.now();
  await page
    .locator("#mirror")
    .click({ position: { x: b.x * 0.75, y: b.y * 0.75 } });
  frame = await wait(newest, (f) => labeled(f, "Host state: 1 clicks"));
  await wait(
    () => page.locator("#revision").textContent(),
    (s) => s !== observed,
  );
  const clientToHostMs = performance.now() - started;
  const e = target(frame, "Editable text");
  await page
    .locator("#mirror")
    .click({ position: { x: e.x * 0.75, y: e.y * 0.75 } });
  await page.locator("#draft:enabled").waitFor();
  const value = "Browser A𝄞éאב";
  await page.locator("#draft").fill(value);
  await page.locator("#apply").click();
  frame = await wait(newest, (f) => hasText(f, value));
  await page
    .locator("#edit-status")
    .filter({ hasText: "Host applied" })
    .waitFor();
  const revision = await page.locator("#revision").textContent();
  const hostStarted = performance.now();
  const currentButton = target(await newest(), "Increment host counter");
  native("click", currentButton.x, currentButton.y);
  frame = await wait(newest, (f) => labeled(f, "Host state: 2 clicks"));
  await wait(
    () => page.locator("#revision").textContent(),
    (s) => s !== revision,
  );
  const hostToBrowserMs = performance.now() - hostStarted;
  native("click", e.x, e.y);
  native("keys", "+Control_L", "a", "-Control_L", "h", "o", "s", "t");
  frame = await wait(newest, (f) => hasText(f, "host"));
  await wait(
    () => page.locator("#draft").inputValue(),
    (s) => s === "host",
  );
  const beforeReconnect = await page.locator("#revision").textContent();
  await page.locator("#disconnect").click();
  await new Promise((done) => setTimeout(done, 400));
  await page.locator("#connect").click();
  await page
    .locator("#revision")
    .filter({ hasText: /Revision/ })
    .waitFor();
  await wait(
    () => page.locator("#connection").textContent(),
    (s) => s === "Observer",
  );
  assert.equal(host.exitCode, null);
  assert.equal(await page.locator("#revision").textContent(), beforeReconnect);
  let hiddenTab = false;
  if (headed) {
    await page.locator("#control").click();
    await page
      .locator("#connection")
      .filter({ hasText: "Controller" })
      .waitFor();
    const prior = await page.locator("#revision").textContent();
    const popup = page.context().waitForEvent("page");
    await page.evaluate(() => window.open("about:blank", "_blank"));
    const cover = await popup;
    await cover.bringToFront();
    await page.waitForFunction(
      () => document.visibilityState === "hidden",
      null,
      { polling: 100 },
    );
    await wait(
      () => page.locator("#connection").textContent(),
      (s) => s === "Observer",
    );
    const current = target(await newest(), "Increment host counter");
    native("click", current.x, current.y);
    await wait(newest, (f) => labeled(f, "Host state: 3 clicks"));
    await new Promise((done) => setTimeout(done, 400));
    assert.equal(
      await page.locator("#revision").textContent(),
      prior,
      "hidden page claimed new presentation",
    );
    assert.equal(
      await page.locator("#connection").textContent(),
      "Observer",
      "hidden page retained control",
    );
    await page.bringToFront();
    await page.waitForFunction(() => document.visibilityState === "visible");
    await wait(
      () => page.locator("#revision").textContent(),
      (s) => s !== prior,
    );
    await cover.close();
    hiddenTab = true;
  }
  assert.deepEqual(errors, []);
  await mkdir(resolve(root, "client-browser/test-output"), { recursive: true });
  await page.screenshot({
    path: resolve(root, "client-browser/test-output/demo.png"),
    fullPage: true,
  });
  const draw = await page.locator("#timing").textContent();
  console.log(
    JSON.stringify({
      passed: true,
      observer: true,
      browserActivation: true,
      unicodeEdit: true,
      hostEdit: true,
      reconnect: true,
      hiddenTab,
      https: secure,
      productionBundle: built,
      listen: "0.0.0.0",
      clientToHostMs,
      hostToBrowserMs,
      draw,
    }),
  );
  host.stdin.write("\n");
  await wait(
    () => host.exitCode,
    (v) => v !== null,
  );
  assert.equal(host.exitCode, 0, logs);
} catch (error) {
  console.error(logs.slice(-5000));
  try {
    const ui =
      browser &&
      (await browser
        .contexts()[0]
        ?.pages()[0]
        ?.locator("#status")
        .textContent());
    console.error("UI: " + ui);
    const f = await newest();
    console.error(
      JSON.stringify({
        revision: f.revision,
        gaps: f.gaps,
        semantics: f.semantics,
        surfaces: f.surfaces.map((s) => ({ key: s.surface_key, gaps: s.gaps })),
      }),
    );
  } catch {}
  throw error;
} finally {
  await browser?.close();
  for (const p of processes.reverse()) {
    if (p.exitCode !== null || p.signalCode !== null) continue;
    const stopped = new Promise((done) => p.once("exit", done));
    p.kill("SIGTERM");
    await Promise.race([
      stopped,
      new Promise((done) => setTimeout(done, 1000)),
    ]);
    if (p.exitCode === null && p.signalCode === null) {
      p.kill("SIGKILL");
      await stopped;
    }
  }
  await rm(temporary, { recursive: true, force: true, maxRetries: 10 });
}
