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
  if (req.url === "/image.svg") { res.setHeader("Content-Type", "image/svg+xml"); res.end('<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="blue"/></svg>'); return; }
  res.setHeader("Content-Type", "text/html");
  res.end(
    `<!doctype html><title>Live browser fixture</title><style>h1{color:rgb(12,34,56)}</style><img id=live-image src=/image.svg alt="Fixture image"><main><h1>Host fixture</h1><p>A readable page on your phone.</p><ul><li>First mobile item</li><li>Second mobile item</li></ul><p>&lt;img src=x onerror=window.sourceInjected=true&gt;</p></main><label>Name<input id=name value=Initial></label><label>Password<input id=secret type=password></label><button onclick="document.getElementById('result').textContent='Action confirmed';document.querySelector('style').textContent='h1{color:rgb(65,43,21)}'">Run action</button><p id=result>Waiting</p><button onclick="if(document.getElementById('secret').value==='not-in-publication'){document.cookie='signedin=yes; SameSite=Lax; Max-Age=86400';document.getElementById('login').textContent='Login accepted';}">Sign in</button><p id=login>${req.headers.cookie?.includes("signedin=yes") ? "Session remembered" : "Signed out"}</p><label>Reviewed<input type=checkbox></label><a href=/next>Next page</a><canvas id=live-canvas width=40 height=40></canvas><script>const ctx=document.querySelector("canvas").getContext("2d");function paint(){ctx.fillStyle="rgb(255,"+(Math.floor(performance.now()/30)%255)+",0)";ctx.fillRect(0,0,40,40);requestAnimationFrame(paint)}paint();</script>`,
  );
});
await new Promise((r) => fixture.listen(0, "127.0.0.1", r));
const socket = createServer();
await new Promise((r) => socket.listen(0, "127.0.0.1", r));
const port = socket.address().port;
await new Promise((r) => socket.close(r));
let origin = `https://localhost:${port}`;
const phone = process.env.RUI_TEST_PHONE === "1";
const blockedIce = process.env.RUI_TEST_BLOCK_ICE === '1';
const proxied = process.env.RUI_TEST_PROXY === "1";
const mount = proxied
  ? (process.env.RUI_TEST_PATH || "").replace(/\/$/, "")
  : "";
let publicURL = origin + mount;
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
const desktopBrowser = process.env.RUI_TEST_DESKTOP_BROWSER === "1";
if (phone && (proxied || desktopBrowser)) throw Error("Phone test uses the standalone browser and real Tailscale Serve");
const startHost = () => {
  let browserShared = false;
  logs = "";
  host = spawn(
    "python3",
    [
      desktopBrowser ? "apps/desktop/run.py" : "apps/browser/run.py",
      ...(desktopBrowser ? ["--with-browser"] : []),
      "--skip-build",
      ...(process.env.RUI_TEST_HEADED === "1" ? [] : ["--headless"]),
      ...(process.env.RUI_TEST_SOURCE_EXECUTABLE ? ["--chromium", process.env.RUI_TEST_SOURCE_EXECUTABLE] : []),
      ...(phone ? ["--phone"] : ["--port", String(backendPort)]),
      ...(phone ? [] : proxied
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
    if (
      desktopBrowser &&
      !browserShared &&
      logs.includes("Open on your phone:")
    ) {
      browserShared = true;
      host.stdin.write("share browser\n");
    }
  });
};
try {
  startHost();
  const invitation = await deadline(
    () => /Open on your phone: (https:\/\/[^\s]+)/.exec(logs)?.[1],
    "host startup",
    40000,
  );
  if (phone) { origin = new URL(invitation).origin; publicURL = origin; }
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
    ignoreHTTPSErrors: !phone,
    viewport: { width: 390, height: 844 },
    hasTouch: true,
  });
  await context.addInitScript(blockedIce => {
    window.ruiUnhandled = [];
    addEventListener('unhandledrejection', event => window.ruiUnhandled.push(String(event.reason)));
    window.ruiSockets = [];
    const Socket = window.WebSocket;
    window.WebSocket = class extends Socket {
      constructor(...args) { super(...args); window.ruiSockets.push(this); }
    };
    window.ruiPeers = [];
    const Peer = window.RTCPeerConnection;
    if (Peer) window.RTCPeerConnection = class extends Peer {
      constructor(options) { super(blockedIce ? { ...options, iceTransportPolicy: 'relay', iceServers: [] } : options); window.ruiPeers.push(this); }
    };
    const clone = window.structuredClone;
    const copies = new Map();
    window.ruiStateCopies = copies;
    window.structuredClone = (value, options) => {
      if (value?.records instanceof Map && typeof value.revision === "bigint")
        copies.set(value.revision, (copies.get(value.revision) ?? 0) + 1);
      return clone(value, options);
    };
  }, blockedIce);
  page = await context.newPage();
  let latestMediaRead, retainedHttp;
  {
    page.on('response', response => {
      if (!response.url().endsWith('/api/media') || !response.ok()) return;
      const request = response.request(), data = request.postDataJSON();
      if (data.op === 'read') latestMediaRead = { time: Date.now(), url: response.url(), headers: request.headers(), token: data.token };
    });
    await page.route('**/api/media', route => route.request().postDataJSON().op === 'close' && retainedHttp ? route.fulfill({ status: 200, contentType: 'application/json', body: '{"ok":true}' }) : route.continue());
  }
  let corruptedResource = false;
  if (!desktopBrowser) await page.route('**/api/document-resource', async route => {
    if (corruptedResource) return route.continue();
    const response = await route.fetch();
    const data = await response.json();
    const bodies = data.bodies || data;
    const key = Object.keys(bodies).find(key => typeof bodies[key] === 'string' && bodies[key].length);
    if (!key) return route.fulfill({ response });
    corruptedResource = true;
    bodies[key] = (bodies[key][0] === 'x' ? 'y' : 'x') + bodies[key].slice(1);
    await route.fulfill({ response, json: data });
  });
  const retainMediaPeer = async () => {
    if (await page.evaluate(() => ['video/webm;codecs=vp8,opus', 'video/webm;codecs=vp8', 'audio/webm;codecs=opus'].some(type => globalThis.MediaSource?.isTypeSupported(type)))) {
      const start = Date.now();
      await deadline(() => latestMediaRead?.time >= start, 'HTTPS media is active');
      retainedHttp = latestMediaRead;
      assert.equal((await page.request.post(retainedHttp.url, { headers: retainedHttp.headers, data: { op: 'renew', token: retainedHttp.token } })).status(), 200);
      return;
    }
    await page.waitForFunction(() => window.ruiPeers.some(peer => peer.connectionState === 'connected'));
    await page.evaluate(async () => {
      const peer = window.ruiPeers.findLast(peer => peer.connectionState === 'connected');
      window.retainedMediaPeer = peer;
      peer.close = () => {};
      window.mediaPacketCount = async () => [...(await peer.getStats()).values()].filter(s => s.type === 'inbound-rtp').reduce((n, s) => n + (s.packetsReceived || 0), 0);
      window.beforeRevokePackets = await window.mediaPacketCount();
    });
    await page.waitForFunction(async () => await window.mediaPacketCount() > window.beforeRevokePackets, null, { timeout: 5000 });
  };
  const assertMediaRetired = async () => {
    if (retainedHttp) {
      for (const op of ['renew', 'read']) assert.equal((await page.request.post(retainedHttp.url, { headers: retainedHttp.headers, data: { op, token: retainedHttp.token, after: 0 } })).status(), 400, 'Retired viewers cannot renew or read HTTPS media even if they omit close');
      retainedHttp = null;
      return;
    }
    await new Promise(resolve => setTimeout(resolve, 500));
    const packets = await page.evaluate(() => window.mediaPacketCount());
    await new Promise(resolve => setTimeout(resolve, 1000));
    assert.equal(await page.evaluate(() => window.mediaPacketCount()), packets, 'host retires media even when the client keeps its peer open');
  };
  const errors = [];
  let retiringView = false;
  page.on("pageerror", e => {
    if (engine === 'webkit' && retiringView && e.message.includes(new URL(origin).host) && e.message.endsWith('/api/media due to access control checks.')) return;
    errors.push(e.message);
  });
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
  const initialCopies = await page.evaluate(() => [...window.ruiStateCopies.values()]);
  assert.ok(
    initialCopies.length > 0 && initialCopies.every(count => count === 1),
    "each publication and control acquisition share one state copy per revision",
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
  if (!desktopBrowser) {
    const replica = page.frameLocator('iframe[title="Host web page"]');
    await replica.getByRole("heading", { name: "Host fixture" }).waitFor();
    assert.equal(await replica.locator("script").count(), 0);
    await deadline(async () => replica.locator("#live-image").evaluate(n => n.complete && n.naturalWidth === 32), "authenticated image resource");
    assert.equal(await replica.locator("h1").evaluate(n => getComputedStyle(n).color), "rgb(12, 34, 56)");
    assert.equal(corruptedResource, true, "viewer recovers after rejecting a corrupt resource");
    await deadline(async () => replica.locator("#live-canvas").evaluate(n => n.getContext("2d").getImageData(10,10,1,1).data[0] > 200), "scoped canvas media");
    if (latestMediaRead) assert.equal(await page.evaluate(() => window.ruiPeers.length), 0, 'HTTPS media starts without a separate peer connection');
    await replica.getByRole("button", { name: "Run action", exact: true }).click();
    await replica.locator("#result").filter({ hasText: "Action confirmed" }).waitFor();
    await deadline(async () => (await replica.locator("h1").evaluate(n => getComputedStyle(n).color)) === "rgb(65, 43, 21)", "stylesheet replacement");
  }
  assert.ok(await page.getByRole("list").count());
  const more = page.locator('[data-presentation="disclosure"] > summary');
  assert.equal(
    await page
      .getByRole("button", { name: "New tab", exact: true })
      .isVisible(),
    false,
  );
  await more.click();
  assert.equal(
    await page
      .getByRole("button", { name: "New tab", exact: true })
      .isVisible(),
    true,
  );
  await more.click();
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
  await page.evaluate(() => {
    const heading = document.querySelector("#view h1");
    const input = [...document.querySelectorAll("#view input")].find(
      (el) => el.value === "Mobile café Ω",
    );
    const range = document.createRange();
    range.selectNodeContents(heading.firstChild);
    input.focus();
    input.setSelectionRange(2, 5);
    window.renderProbe = { heading, text: heading.firstChild, input, range };
    document.getElementById("release").click();
  });
  await deadline(
    async () =>
      await page
        .getByRole("button", { name: "Run action", exact: true })
        .isDisabled(),
    "release disables actions",
  );
  assert.deepEqual(
    await page.evaluate(() => {
      const { heading, text, input, range } = window.renderProbe;
      return {
        sameText: heading.firstChild === text,
        selected: range.toString(),
        focused: document.activeElement === input,
        cursor: [input.selectionStart, input.selectionEnd],
      };
    }),
    { sameText: true, selected: "Host fixture", focused: true, cursor: [2, 5] },
  );
  retiringView = true;
  if (!desktopBrowser) {
    await retainMediaPeer();
    await page.evaluate(() => window.ruiSockets.filter(socket => socket.readyState === WebSocket.OPEN).forEach(socket => socket.close()));
    await deadline(async () => (await page.locator('#connection').textContent()) === 'Disconnected', 'view transport retirement');
    await assertMediaRetired();
  }
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
  retiringView = false;
  await page.getByRole("button", { name: "Run action", exact: true }).click();
  await deadline(
    async () =>
      (await page.locator("#status").textContent()).includes("Action sent"),
    "new action after reload",
  );
  const stop = new Promise((resolve) => host.once("exit", resolve));
  retiringView = true;
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
  retiringView = false;
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
  if (!desktopBrowser) await retainMediaPeer();
  retiringView = true;
  host.stdin.write(`revoke ${deviceId}\n`);
  await deadline(
    async () =>
      (await page.locator("#connection").textContent()) === "Disconnected",
    "local revocation disconnect",
  );
  if (!desktopBrowser) await assertMediaRetired();
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
  assert.deepEqual(await page.evaluate(() => window.ruiUnhandled), []);
  const report = {
    engine,
    reverseProxy: proxied,
    tailscalePhone: phone,
    mediaTransport: latestMediaRead ? 'https' : 'webrtc',
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
    documentReplica: !desktopBrowser,
    hashedResources: !desktopBrowser,
    scopedCanvas: !desktopBrowser,
    mediaRetirement: !desktopBrowser,
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
    console.error("Media diagnostics:", JSON.stringify(await page.evaluate(async () => ({
      peers: await Promise.all((window.ruiPeers || []).map(async peer => ({ state: peer.connectionState, remoteMdns: peer.remoteDescription?.sdp.includes('.local'), stats: [...(await peer.getStats()).values()].filter(s => s.type === 'inbound-rtp').map(s => ({ kind: s.kind, packets: s.packetsReceived, frames: s.framesDecoded })) }))),
      unhandled: window.ruiUnhandled,
      decoders: [...document.querySelectorAll('video')].map(video => ({ ready: video.readyState, paused: video.paused }))
    }))));
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
