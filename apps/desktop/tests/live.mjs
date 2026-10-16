// Full production Rust/TLS transport + private GTK source + phone-sized client.
import {
  chromium,
  webkit,
} from "../../browser/node_modules/playwright/index.mjs";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, rm, readFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import { native } from "../workspace.mjs";
import { reverseProxy } from "../../browser/tests/reverse-proxy.mjs";

assert.equal(process.env.RUI_ISOLATED_DISPLAY, "1");
const root = resolve(import.meta.dirname, "../../..");
const directory = await mkdtemp(join(tmpdir(), "rui-desktop-live-"));
const saved = join(directory, "document.txt");
let source,
  second,
  picker,
  pickerSource,
  host,
  browser,
  proxy,
  page,
  logs = "",
  sourceLogs = "";
const engine = process.env.RUI_CLIENT_ENGINE || "chromium";
const deadline = async (fn, label, ms = 20000) => {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const result = await fn();
    if (result) return result;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error(label);
};
const port = async () => {
  const server = createServer();
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const value = server.address().port;
  await new Promise((r) => server.close(r));
  return value;
};
const stop = async (process) => {
  if (!process || process.exitCode !== null) return;
  process.kill("SIGTERM");
  await deadline(
    () => process.exitCode !== null || process.signalCode !== null,
    "process stop",
    10000,
  );
};
try {
  source = spawn("/usr/bin/python3", ["apps/desktop/tests/source.py", saved], {
    cwd: root,
    stdio: ["pipe", "pipe", "pipe"],
  });
  source.stdout.on("data", (d) => (sourceLogs += d));
  source.stderr.on("data", () => {});
  await deadline(
    () => sourceLogs.includes('"ready": true'),
    "existing GTK app startup",
  );
  second = spawn(
    "/usr/bin/python3",
    ["apps/desktop/tests/source.py", join(directory, "second.txt"), "B"],
    { cwd: root, stdio: ["pipe", "pipe", "pipe"] },
  );
  let secondLogs = "";
  second.stdout.on("data", (d) => (secondLogs += d));
  second.stderr.on("data", () => {});
  await deadline(
    () => secondLogs.includes('"ready": true'),
    "second existing GTK app",
  );
  // Both apps are already open before we start the host.
  const cert = join(directory, "cert.pem"),
    key = join(directory, "key.pem");
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
      "subjectAltName=DNS:localhost",
      "-keyout",
      key,
      "-out",
      cert,
    ],
    { stdio: "ignore" },
  );
  const external = await port(),
    backend = await port();
  const origin = `https://localhost:${external}`,
    mount = "/desktop";
  proxy = await reverseProxy({
    cert,
    key,
    port: external,
    backendPort: backend,
    mount,
  });
  host = spawn(
    "python3",
    [
      "apps/desktop/run.py",
      "--skip-build",
      "--public-url",
      origin + mount,
      "--port",
      String(backend),
      "--state-dir",
      join(directory, "state"),
    ],
    { cwd: root, stdio: ["pipe", "pipe", "pipe"] },
  );
  host.stdout.on("data", (d) => (logs += d));
  host.stderr.on("data", (d) => (logs += d));
  const invitation = await deadline(
    () => /Open on your phone: (https:\/\/[^\s]+)/.exec(logs)?.[1],
    "host startup",
    40000,
  );
  browser =
    engine === "webkit"
      ? await webkit.launch({
          headless: true,
          executablePath: process.env.RUI_WEBKIT_EXECUTABLE,
        })
      : await chromium.launch({
          executablePath: "/usr/bin/chromium",
          headless: true,
          args: ["--no-sandbox"],
        });
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    ignoreHTTPSErrors: true,
  });
  page = await context.newPage();
  page.setDefaultTimeout(10000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(invitation);
  const code = await deadline(
    () => /Approve locally: approve ([a-f0-9]+)/.exec(logs)?.[1],
    "pairing code",
  );
  host.stdin.write(`approve ${code}\n`);
  await page
    .getByRole("button", { name: "Take control", exact: true })
    .waitFor();
  await deadline(
    () =>
      page
        .getByRole("button", { name: "Take control", exact: true })
        .isEnabled(),
    "paired connection",
  );
  assert.equal(await page.getByLabel("Document", { exact: true }).count(), 0);
  assert.equal(await page.title(), "RUI Desktop");
  host.stdin.write("apps\n");
  await deadline(
    () => logs.includes(`"pid": ${source.pid}`),
    "local app discovery",
  );
  const apps = (await native({ op: "list" })).apps;
  const app = apps.find((a) => a.pid === source.pid);
  const secondApp = apps.find((a) => a.pid === second.pid);
  const identifier = (app) =>
    createHash("sha256")
      .update(JSON.stringify([app.root, app.pid, app.start]))
      .digest("hex")
      .slice(0, 16);
  const id = identifier(app),
    secondId = identifier(secondApp);
  picker = spawn(
    "/usr/bin/python3",
    ["apps/desktop/share.py", "--state-dir", join(directory, "state")],
    { cwd: root, stdio: ["ignore", "ignore", "pipe"] },
  );
  picker.stderr.on("data", () => {});
  const owner = await deadline(
    async () =>
      (await native({ op: "list" })).apps.find((a) => a.pid === picker.pid),
    "local picker",
  );
  pickerSource = owner;
  assert.equal(
    owner.local_only,
    true,
    "kernel process marker also survives host restart",
  );
  const ownerAction = async (label) => {
    const capture = await native({ op: "capture", source: owner });
    const byKey = new Map(capture.nodes.map((n) => [JSON.stringify(n.key), n]));
    const identity = `${app.name} (${app.pid})`;
    const button = capture.nodes.find((n) => {
      if (n.label !== label || !n.actions.includes("Activate")) return false;
      let parent = n.parent && byKey.get(JSON.stringify(n.parent));
      while (parent) {
        if (
          parent.children.some(
            (k) => byKey.get(JSON.stringify(k))?.label === identity,
          )
        )
          return true;
        parent = parent.parent && byKey.get(JSON.stringify(parent.parent));
      }
      return false;
    });
    assert.ok(button, label);
    assert.equal(
      (
        await native({
          op: "dispatch",
          source: owner,
          key: button.key,
          fingerprint: button.fingerprint,
          action: "Activate",
          expires: Date.now() + 2000,
        })
      ).status,
      "dispatched",
    );
  };
  await deadline(
    async () =>
      (await native({ op: "capture", source: owner })).nodes.some(
        (n) => n.label === `${app.name} (${app.pid})`,
      ),
    "picker discovery",
  );
  const localApps = JSON.parse(
    execFileSync(
      "/usr/bin/python3",
      [
        "apps/desktop/share.py",
        "--state-dir",
        join(directory, "state"),
        "--list",
      ],
      { encoding: "utf8", timeout: 10000 },
    ),
  ).apps;
  assert.equal(
    localApps.some((a) => a.pid === picker.pid),
    false,
    "owner picker must never be shareable",
  );
  await ownerAction("Check support");
  await deadline(
    async () =>
      (await native({ op: "capture", source: owner })).nodes.some((n) =>
        n.label.includes("Support checked;"),
      ),
    "local diagnostics",
  );
  assert.equal(
    await page.getByLabel("Document", { exact: true }).count(),
    0,
    "diagnostics do not share content",
  );
  assert.equal(
    await page.evaluate(
      async () =>
        (
          await fetch(new URL("api/desktop", document.baseURI), {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ command: "apps" }),
          })
        ).status,
    ),
    404,
    "no remote sharing endpoint",
  );
  await ownerAction("Share");
  const document = page.getByLabel("Document", { exact: true });
  await document.waitFor();
  assert.equal(await document.inputValue(), "Already open on the host");
  assert.equal(
    (await page.content()).includes("never-publish-desktop-secret"),
    false,
  );
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await deadline(
    () =>
      page.getByRole("button", { name: "Release", exact: true }).isEnabled(),
    "control grant",
  );
  host.stdin.write(`share ${secondId}\n`);
  await deadline(
    async () =>
      (await document.count()) > 0 &&
      (await document.inputValue()) === "Second existing application",
    "second app selected",
  );
  await page.getByRole("button", { name: /^Open:/ }).click();
  await deadline(
    async () =>
      (await document.count()) > 0 &&
      (await document.inputValue()) === "Already open on the host",
    "switch back to first app",
  );
  host.stdin.write(`unshare ${secondId}\n`);
  await deadline(
    async () =>
      (await page.getByRole("button", { name: /^Open:/ }).count()) === 0,
    "second app unshared",
  );
  await deadline(
    async () => (await document.count()) > 0 && (await document.isEditable()),
    "first app editable",
  );
  source.stdin.write('{"op":"window"}\n');
  const otherWindow = page.getByRole("button", {
    name: "Show window: Second notes window",
    exact: true,
  });
  await otherWindow.waitFor();
  await otherWindow.click();
  await page
    .getByText("Content in the second window", { exact: true })
    .waitFor();
  assert.equal(await document.count(), 0);
  source.stdin.write('{"op":"close-window"}\n');
  await document.waitFor();
  await deadline(() => document.isEditable(), "closed window fallback");
  const edit = "From phone — 東京 👋";
  await document.fill(edit);
  await document
    .locator("..")
    .getByRole("button", { name: "Apply", exact: true })
    .click();
  const save = page.getByRole("button", { name: "Save document", exact: true });
  await deadline(() => save.isEnabled(), "edit result");
  await save.click();
  await deadline(
    async () => (await readFile(saved, "utf8").catch(() => "")) === edit,
    "native saved file",
  );
  // Host-side edits must invalidate, not overwrite, the phone's pending draft.
  await document.fill("Unsent phone draft");
  source.stdin.write('{"op":"text","value":"Changed on desktop"}\n');
  await page
    .getByText(
      "The source changed. Your draft has been kept; discard it to edit the new value.",
      { exact: true },
    )
    .waitFor();
  assert.equal(await document.inputValue(), "Unsent phone draft");
  await document
    .locator("..")
    .getByRole("button", { name: "Discard", exact: true })
    .click();
  assert.equal(await document.inputValue(), "Changed on desktop");
  source.stdin.write('{"op":"modal"}\n');
  const dismiss = page.getByRole("button", {
    name: "Dismiss dialog",
    exact: true,
  });
  await dismiss.waitFor();
  assert.equal(await save.count(), 0);
  await dismiss.click();
  await save.waitFor();
  await deadline(() => save.isEnabled(), "modal closed");
  source.stdin.write('{"op":"controls"}\n');
  await page
    .getByRole("button", {
      name: "Show window: Semantic controls",
      exact: true,
    })
    .click();
  const volume = page.getByLabel("Volume", { exact: true });
  await volume.waitFor();
  await deadline(() => volume.isEditable(), "numeric control editable");
  await volume.fill("42.5");
  await volume
    .locator("..")
    .getByRole("button", { name: "Apply", exact: true })
    .click();
  const controlState = async () =>
    JSON.parse(await readFile(saved + ".controls.json", "utf8"));
  await deadline(
    async () => (await controlState()).volume === 42.5,
    "native numeric value",
  );
  await page.getByRole("button", { name: "Select: Beta", exact: true }).click();
  await deadline(
    async () => (await controlState()).selected === 1,
    "native list selection",
  );
  await page
    .getByRole("button", { name: "Select: Second tab", exact: true })
    .click();
  await deadline(
    async () => (await controlState()).tab === 1,
    "native tab selection",
  );
  await page
    .getByRole("button", { name: "Scroll forward", exact: true })
    .click();
  await deadline(
    async () => (await controlState()).scroll > 0,
    "native scroll",
  );
  await page
    .getByRole("button", {
      name: "Show window: Existing desktop notes",
      exact: true,
    })
    .click();
  await document.waitFor();
  const out = resolve(root, "apps/desktop/test-output");
  await mkdir(out, { recursive: true });
  await page.screenshot({
    path: join(out, `${engine}-mobile.png`),
    fullPage: true,
  });
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "mobile horizontal overflow",
  );
  await page.reload();
  await deadline(
    () =>
      page
        .getByRole("button", { name: "Take control", exact: true })
        .isEnabled(),
    "reconnect",
  );
  assert.equal(await document.inputValue(), "Changed on desktop");
  host.stdin.write(`unshare ${id}\n`);
  await deadline(
    async () => (await document.count()) === 0,
    "unshare clears content",
  );
  host.stdin.write(`share ${id}\n`);
  await document.waitFor();
  assert.equal(await document.inputValue(), "Changed on desktop");
  const client = await deadline(
    () => /Device approved: ([a-f0-9]+)/.exec(logs)?.[1],
    "approved device",
  );
  host.stdin.write(`revoke ${client}\n`);
  await deadline(
    async () => (await document.count()) === 0,
    "device revoke clears content",
  );
  assert.deepEqual(errors, []);
  host.stdin.write("quit\n");
  await deadline(() => host.exitCode !== null, "host stopped");
  assert.equal(source.exitCode, null, "attached app survives host shutdown");
  console.log(
    JSON.stringify({
      engine,
      mobile: "390x844 touch",
      mounted_path: mount,
      existing_app: true,
      local_sharing_picker: true,
      private_diagnostics: true,
      app_switching: true,
      window_navigation: true,
      closed_window_recovery: true,
      edit_save: true,
      native_selection_values_scroll: true,
      stale_draft: true,
      modal: true,
      reconnect: true,
      unshare: true,
      revoke: true,
      source_survives: true,
    }),
  );
} catch (error) {
  if (pickerSource)
    console.error(
      "Picker:",
      (
        await native({ op: "capture", source: pickerSource }).catch(() => ({
          nodes: [],
        }))
      ).nodes.map((n) => n.label),
    );
  console.error(
    "Client:",
    await page
      ?.locator("body")
      .innerText()
      .catch(() => "unavailable"),
  );
  console.error("Saved:", await readFile(saved, "utf8").catch(() => "no file"));
  console.error(logs.slice(-800));
  throw error;
} finally {
  await browser?.close();
  await stop(picker);
  await stop(host);
  await stop(source);
  await stop(second);
  await proxy?.close();
  await rm(directory, { recursive: true, force: true });
}
