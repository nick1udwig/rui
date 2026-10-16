// Production assets, real TLS/WSS, Rust authority and an owned Chromium source.
import { chromium, firefox, webkit } from "playwright";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createServer } from "node:http";
import assert from "node:assert/strict";
import { reverseProxy } from "./reverse-proxy.mjs";

const root = resolve(import.meta.dirname, "../../.."),
  directory = await mkdtemp(join(tmpdir(), "rui-live-"));
const out = resolve(import.meta.dirname, "../test-output");
await mkdir(out, { recursive: true });
const cert = join(directory, "web.pem"),
  key = join(directory, "web-key.pem");
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
let host,
  browser,
  page,
  logs = "";
const fixture = createServer((req, res) => {
  res.setHeader("Content-Type", "text/html");
  res.end(
    `<!doctype html><title>Live browser fixture</title><main><h1>Host fixture</h1><p>A readable page on your phone.</p><ul><li>First mobile item</li><li>Second mobile item</li></ul><p>&lt;img src=x onerror=window.sourceInjected=true&gt;</p></main><label>Name<input id=name value=Initial></label><label>Password<input id=secret type=password></label><button onclick="document.getElementById('result').textContent='Action confirmed'">Run action</button><p id=result>Waiting</p><button onclick="if(document.getElementById('secret').value==='not-in-publication'){document.cookie='signedin=yes; SameSite=Lax; Max-Age=86400';document.getElementById('login').textContent='Login accepted';}">Sign in</button><p id=login>${req.headers.cookie?.includes("signedin=yes") ? "Session remembered" : "Signed out"}</p><label>Reviewed<input type=checkbox></label><a href=/next>Next page</a>`,
  );
});
await new Promise((r) => fixture.listen(0, "127.0.0.1", r));
const socket = createServer();
await new Promise((r) => socket.listen(0, "127.0.0.1", r));
const port = socket.address().port;
await new Promise((r) => socket.close(r));
const origin = `https://localhost:${port}`;
const proxied = process.env.RUI_TEST_PROXY === "1";
const mount = proxied
  ? (process.env.RUI_TEST_PATH || "").replace(/\/$/, "")
  : "";
const publicURL = origin + mount;
let proxy,
  backendPort = port;
if (proxied) {
  const backendSocket = createServer();
  await new Promise((r) => backendSocket.listen(0, "127.0.0.1", r));
  backendPort = backendSocket.address().port;
  await new Promise((r) => backendSocket.close(r));
  proxy = await reverseProxy({ cert, key, port, backendPort, mount });
}
const engine = process.env.RUI_CLIENT_ENGINE || "chromium";
const deadline = async (fn, message, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const result = await fn();
    if (result) return result;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error(message);
};
const startHost = () => {
  logs = "";
  host = spawn(
    "python3",
    [
      "apps/browser/run.py",
      "--skip-build",
      "--headless",
      "--port",
      String(backendPort),
      ...(proxied
        ? ["--public-url", publicURL]
        : ["--host", "localhost", "--cert", cert, "--key", key]),
      "--files-dir",
      directory,
      "--state-dir",
      join(directory, "state"),
    ],
    { cwd: root, stdio: ["pipe", "pipe", "pipe"] },
  );
  host.stdout.on("data", (d) => (logs += d));
  host.stderr.on("data", (d) => {
    logs += d;
  });
};
try {
  startHost();
  const invitation = await deadline(
    () => /Open on your phone: (https:\/\/[^\s]+)/.exec(logs)?.[1],
    "host startup",
    40000,
  );
  assert.equal(new URL(invitation).origin, origin);
  assert.equal(new URL(invitation).pathname, mount + "/");
  if (proxied) {
    const check = (headers, path = "/", method = "GET") =>
      new Promise((resolve, reject) => {
        import("node:http").then(({ request }) => {
          const req = request(
            { host: "127.0.0.1", port: backendPort, path, method, headers },
            (res) => {
              res.resume();
              res.on("end", () => resolve(res.statusCode));
            },
          );
          req.on("error", reject);
          req.end();
        });
      });
    assert.equal(await check({ Host: new URL(origin).host }), 200);
    assert.equal(
      await check({
        Host: "wrong.example",
        "X-Forwarded-Host": new URL(origin).host,
      }),
      403,
    );
    assert.equal(
      await check(
        {
          Host: new URL(origin).host,
          Origin: "https://wrong.example",
          "Content-Type": "application/json",
        },
        "/api/pair",
        "POST",
      ),
      403,
    );
  }
  browser = await { chromium, firefox, webkit }[engine].launch({
    headless: true,
    ...(engine === "chromium"
      ? { executablePath: "/usr/bin/chromium", chromiumSandbox: true }
      : {}),
    ...(process.env.RUI_CLIENT_EXECUTABLE
      ? { executablePath: process.env.RUI_CLIENT_EXECUTABLE }
      : {}),
  });
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    viewport: { width: 390, height: 844 },
    hasTouch: true,
  });
  page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error") console.error("client:", m.text());
  });
  if (mount) {
    await page.goto(publicURL + "/");
    await page.evaluate(
      () =>
        new Promise((resolve, reject) => {
          const open = indexedDB.open("rui-browser", 1);
          open.onupgradeneeded = () => open.result.createObjectStore("devices");
          open.onsuccess = () => {
            const db = open.result;
            const tx = db.transaction("devices", "readwrite");
            tx.objectStore("devices").put(
              { client: "unrelated-workspace" },
              "host",
            );
            tx.oncomplete = () => {
              db.close();
              resolve(true);
            };
            tx.onerror = () => reject(tx.error);
          };
        }),
    );
  }
  // Bare mount URLs must work too: the HTML base preserves all relative URLs.
  await page.goto(
    mount ? invitation.replace(mount + "/#", mount + "#") : invitation,
  );
  assert.equal(
    await page.evaluate(() => new URL(document.baseURI).pathname),
    mount + "/",
  );
  const code = await deadline(
    () => /Approve locally: approve ([a-f0-9]+)/.exec(logs)?.[1],
    "pairing request",
  );
  assert.ok((await page.locator("#pair-status").textContent()).includes(code));
  host.stdin.write(`approve ${code}\n`);
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Take control", exact: true })
        .isDisabled()),
    "connected controls",
  );
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await deadline(
    async () =>
      (await page.locator("#connection").textContent()) === "Controlling host",
    "control lease",
  );
  const address = page.getByLabel("Address", { exact: true });
  await address.fill(`http://127.0.0.1:${fixture.address().port}/`);
  await page.getByRole("button", { name: "Go", exact: true }).click();
  await page.getByRole("button", { name: "Run action", exact: true }).waitFor();
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Run action", exact: true })
        .isDisabled()),
    "page ready",
  );
  await page.getByRole("heading", { name: "Host fixture", level: 1 }).waitFor();
  assert.ok(await page.getByRole("list").count());
  assert.equal(await page.locator("#view img").count(), 0);
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  );
  await page.getByRole("button", { name: "Run action", exact: true }).click();
  await page.getByText("Action confirmed", { exact: true }).waitFor();
  const name = page.getByLabel("Name", { exact: true });
  await name.fill("Mobile café Ω");
  await name
    .locator("..")
    .getByRole("button", { name: "Apply", exact: true })
    .click();
  await deadline(
    async () => (await name.getAttribute("data-dirty")) === "false",
    "text applied",
  );
  await page.screenshot({
    path: join(out, `workspace-${engine}.png`),
    fullPage: true,
  });
  const password = page.getByLabel("Password", { exact: true });
  await password.fill("not-in-publication");
  await password
    .locator("..")
    .getByRole("button", { name: "Apply", exact: true })
    .click();
  await deadline(
    async () => (await password.inputValue()) === "",
    "password cleared",
  );
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Sign in", exact: true })
        .isDisabled()),
    "password dispatched",
  );
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.getByText("Login accepted", { exact: true }).waitFor();
  assert.ok(
    !(await page.locator("#view").textContent()).includes("not-in-publication"),
  );
  await page.getByRole("button", { name: "Release", exact: true }).click();
  await deadline(
    async () =>
      await page
        .getByRole("button", { name: "Run action", exact: true })
        .isDisabled(),
    "release disables actions",
  );
  await page.reload();
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Take control", exact: true })
        .isDisabled()),
    "remembered device reconnect",
  );
  assert.notEqual(
    await page.locator("#connection").textContent(),
    "Controlling host",
  );
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Run action", exact: true })
        .isDisabled()),
    "restored command sequence",
  );
  await page.getByRole("button", { name: "Run action", exact: true }).click();
  await deadline(
    async () =>
      (await page.locator("#status").textContent()).includes("Action sent"),
    "new action after reload",
  );
  const stop = new Promise((resolve) => host.once("exit", resolve));
  host.kill("SIGTERM");
  await stop;
  await deadline(
    async () =>
      (await page.locator("#connection").textContent()) === "Disconnected",
    "disconnect after host shutdown",
  );
  startHost();
  await deadline(
    () => logs.includes("Open on your phone:"),
    "restarted host",
    40000,
  );
  await page.getByRole("button", { name: "Connect", exact: true }).click();
  await deadline(
    async () =>
      !(await page
        .getByRole("button", { name: "Take control", exact: true })
        .isDisabled()),
    "persisted host approval",
  );
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await deadline(
    async () =>
      (await page.locator("#connection").textContent()) === "Controlling host",
    "fresh epoch control",
  );
  await page
    .getByLabel("Address", { exact: true })
    .fill(`http://127.0.0.1:${fixture.address().port}/`);
  await page.getByRole("button", { name: "Go", exact: true }).click();
  await page.getByText("Session remembered", { exact: true }).waitFor();
  const deviceId = await page.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const request = indexedDB.open("rui-browser", 1);
        request.onsuccess = () => {
          const db = request.result;
          const read = db
            .transaction("devices")
            .objectStore("devices")
            .get(
              new URL(document.baseURI).pathname === "/"
                ? "host"
                : `host:${new URL(document.baseURI).pathname}`,
            );
          read.onsuccess = () => {
            resolve(read.result.client);
            db.close();
          };
          read.onerror = () => reject(read.error);
        };
      }),
  );
  host.stdin.write(`revoke ${deviceId}\n`);
  await deadline(
    async () =>
      (await page.locator("#connection").textContent()) === "Disconnected",
    "local revocation disconnect",
  );
  assert.ok(
    await page
      .getByRole("button", { name: "Take control", exact: true })
      .isDisabled(),
  );
  if (mount) {
    assert.ok(
      proxy.requests.every(
        (r) => r.path === mount || r.path.startsWith(mount + "/"),
      ),
      JSON.stringify(proxy.requests),
    );
    assert.ok(
      proxy.requests.some((r) => r.upgrade && r.path === mount + "/rui"),
    );
    for (const extension of [".js", ".css", ".wasm"])
      assert.ok(
        proxy.requests.some((r) => r.path.endsWith(extension)),
        extension,
      );
    const unrelated = await page.evaluate(
      () =>
        new Promise((resolve) => {
          const open = indexedDB.open("rui-browser", 1);
          open.onsuccess = () => {
            const db = open.result;
            const get = db
              .transaction("devices")
              .objectStore("devices")
              .get("host");
            get.onsuccess = () => {
              resolve(get.result.client);
              db.close();
            };
          };
        }),
    );
    assert.equal(unrelated, "unrelated-workspace");
  }
  assert.equal(errors.length, 0, errors.join("\n"));
  const report = {
    engine,
    reverseProxy: proxied,
    mountPath: mount || "/",
    viewport: "390x844",
    pairing: true,
    control: true,
    navigation: true,
    action: true,
    unicode: true,
    password: true,
    reconnect: true,
    hostRestart: true,
    persistentLogin: true,
    revoke: true,
    physicalDevice: false,
  };
  await writeFile(
    join(
      out,
      `live-${engine}${proxied ? "-proxy" : ""}${mount ? "-path" : ""}.json`,
    ),
    JSON.stringify(report, null, 2) + "\n",
  );
  console.log(JSON.stringify(report));
} catch (error) {
  console.error(
    logs
      .split("\n")
      .filter((l) => !l.includes("\u001b") && !l.includes("++++"))
      .join("\n")
      .replace(/invite=[a-f0-9]+/g, "invite=[REDACTED]"),
  );
  if (page) {
    console.error(
      "Client status:",
      await page.locator("#status").textContent(),
    );
    await page.screenshot({ path: join(out, "failure.png"), fullPage: true });
  }
  throw error;
} finally {
  await browser?.close();
  if (host && host.exitCode === null) {
    host.stdin.write("quit\n");
    await Promise.race([
      new Promise((r) => host.once("exit", r)),
      new Promise((r) => setTimeout(r, 8000)),
    ]);
    if (host.exitCode === null) host.kill("SIGTERM");
  }
  await proxy?.close();
  await new Promise((r) => fixture.close(r));
  await rm(directory, { recursive: true, force: true });
}
