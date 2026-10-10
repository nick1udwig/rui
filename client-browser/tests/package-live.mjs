import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createServer } from "node:net";
import { Agent } from "node:https";
import { readFile, readdir, mkdir, writeFile, rm } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { chromium } from "playwright";
import { cpus, release } from "node:os";
import {
  prepareConsumer,
  repository,
  packageRoot,
  sha,
} from "./package-support.mjs";

assert.equal(
  process.env.RUI_ISOLATED_DISPLAY,
  "1",
  "private display/bus required",
);
const prepared = await prepareConsumer();
const temporary = prepared.temporary,
  identity = resolve(temporary, "identity");
const lifecycle = process.env.RUI_LIFECYCLE === "1";
let host,
  browser,
  browserProcess,
  serving,
  logs = "";
const wait = async (read, condition = Boolean) => {
  const deadline = Date.now() + 20000;
  let last;
  while (Date.now() < deadline) {
    const result = await read();
    last = result;
    if (condition(result)) return result;
    await new Promise((done) => setTimeout(done, 25));
  }
  if (last && typeof last === "object") {
    const { state, ...summary } = last;
    console.error(
      JSON.stringify(summary, (_key, value) =>
        typeof value === "bigint" ? value.toString() : value,
      ),
    );
  }
  throw Error("acceptance deadline\n" + logs.slice(-3000));
};
const ready = (s) =>
  s.connection === "connected" &&
  s.revision > 0n &&
  s.presented === s.revision &&
  s.resourcesReady &&
  !s.pending;
const pin = (certificate) => {
  const key = execFileSync("openssl", [
    "x509",
    "-in",
    certificate,
    "-pubkey",
    "-noout",
  ]);
  const der = execFileSync("openssl", ["pkey", "-pubin", "-outform", "DER"], {
    input: key,
  });
  return Buffer.from(sha(der), "hex").toString("base64");
};
async function newest() {
  const names = (await readdir(resolve(identity, "source")))
    .filter((n) => /^whole-\d+\.json$/.test(n))
    .sort();
  return JSON.parse(
    await readFile(resolve(identity, "source", names.at(-1)), "utf8"),
  );
}
function native(operation, ...args) {
  execFileSync(
    "python3",
    [
      resolve(repository, "fixtures/render/native_events.py"),
      "RUI Host",
      operation,
      ...args.map(String),
    ],
    { env: process.env, timeout: 5000 },
  );
}
try {
  const reservation = createServer();
  await new Promise((done) => reservation.listen(0, "127.0.0.1", done));
  const port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  const origin = `https://127.0.0.1:${port}`;
  host = spawn(
    resolve(repository, "target/release/rui-demo"),
    [
      "--host-only",
      "--second-client",
      "--listen",
      "0.0.0.0:0",
      "--identity-dir",
      identity,
      "--browser-origin",
      origin,
    ],
    {
      cwd: repository,
      env: {
        ...process.env,
        ...(lifecycle
          ? {
              RUI_TIMINGS: "16384",
              RUI_TIMINGS_REPORT: resolve(temporary, "host-timings.json"),
              RUI_SOURCE_TIMINGS_REPORT: resolve(
                temporary,
                "source-timings.json",
              ),
            }
          : {}),
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  for (const stream of [host.stdout, host.stderr])
    stream.on("data", (bytes) => (logs = (logs + bytes).slice(-32768)));
  const hostPort = await wait(
    () => /Visible GTK host listening at 0.0.0.0:(\d+)/.exec(logs)?.[1],
  );
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
    { stdio: "ignore" },
  );
  const { preview } = await import(
    pathToFileURL(
      resolve(prepared.consumer, "node_modules/vite/dist/node/index.js"),
    )
  );
  const csp = `default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self' ${origin.replace("https:", "wss:")} wss://127.0.0.1:${hostPort}; style-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'`;
  serving = await preview({
    root: prepared.consumer,
    configFile: false,
    base: "/apps/rui/",
    preview: {
      host: "127.0.0.1",
      port,
      strictPort: true,
      https: { cert: await readFile(cert), key: await readFile(key) },
      headers: { "Content-Security-Policy": csp },
      proxy: {
        "/rui": {
          target: `wss://127.0.0.1:${hostPort}`,
          ws: true,
          secure: true,
          agent: new Agent({
            ca: await readFile(resolve(identity, "host.pem")),
            servername: "rui-demo",
            rejectUnauthorized: true,
          }),
        },
      },
    },
  });
  // The fixture trust exception is limited to the two owned certificates.
  // Real deployments use browser-trusted certificates, not these test pins.
  const browserArgs = [
    "--ignore-certificate-errors-spki-list=" +
      [pin(cert), pin(resolve(identity, "host.pem"))].join(","),
  ];
  if (lifecycle && process.env.RUI_BROWSER_HEADED === "1") {
    browserProcess = spawn(
      process.env.RUI_WASM_CHROMIUM ?? chromium.executablePath(),
      [
        ...browserArgs,
        "--ozone-platform=x11",
        "--remote-debugging-address=127.0.0.1",
        "--remote-debugging-port=0",
        `--user-data-dir=${resolve(temporary, "browser")}`,
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-extensions",
        "about:blank",
      ],
      { env: process.env, stdio: ["ignore", "ignore", "pipe"] },
    );
    browserProcess.stderr.on(
      "data",
      (bytes) => (logs = (logs + bytes).slice(-32768)),
    );
    const endpoint = await wait(
      () =>
        /DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(logs)?.[1],
    );
    browser = await chromium.connectOverCDP(endpoint, { noDefaults: true });
  } else
    browser = await chromium.launch({
      headless: true,
      executablePath: process.env.RUI_WASM_CHROMIUM,
      args: browserArgs,
    });
  const reports = [];
  const credential = [
    ...(await readFile(resolve(identity, "client.credential"))),
  ];
  const secondCredential = [
    ...(await readFile(resolve(identity, "second-client.credential"))),
  ];
  if (lifecycle) {
    const page =
      process.env.RUI_BROWSER_HEADED === "1"
        ? browser.contexts()[0].pages()[0]
        : await browser.newPage();
    await page.setViewportSize({ width: 1200, height: 1200 });
    await page.goto(origin + "/apps/rui/");
    await page.waitForFunction(() => !!window.consumer);
    const { measureLifecycle } = await import("./lifecycle.mjs");
    const measured = await measureLifecycle({
      page,
      browser,
      address: origin.replace("https:", "wss:") + "/rui",
      credential,
      wait,
      ready,
    });
    const browserVersion = browser.version();
    await browser.close();
    browser = undefined;
    host.stdin.write("\n");
    await wait(
      () => host.exitCode,
      (value) => value !== null,
    );
    assert.equal(host.exitCode, 0);
    const result = {
      ...measured,
      packageVersion: prepared.report.version,
      hardware: cpus()[0]?.model,
      kernel: release(),
      browser: process.env.RUI_WASM_CHROMIUM ?? "playwright chromium",
      browserVersion,
      node: process.version,
      rust: execFileSync("rustc", ["--version"], { encoding: "utf8" }).trim(),
      native: execFileSync("pkg-config", ["--modversion", "gtk4", "cairo"], {
        encoding: "utf8",
      })
        .trim()
        .split("\n"),
      viewport: { width: 1200, height: 1200, scale: 1 },
      network:
        "loopback HTTPS/WSS verifying proxy; owned certificate key exception",
      host: JSON.parse(
        await readFile(resolve(temporary, "host-timings.json"), "utf8"),
      ),
      source: JSON.parse(
        await readFile(resolve(temporary, "source-timings.json"), "utf8"),
      ),
    };
    await mkdir(resolve(packageRoot, "test-output"), { recursive: true });
    await writeFile(
      resolve(packageRoot, "test-output/lifecycle.json"),
      JSON.stringify(result) + "\n",
    );
    console.log(
      JSON.stringify({
        passed: true,
        lifecycle: true,
        cycles: measured.rows.lifecycle.length,
        actualHidden: !!measured.hidden,
      }),
    );
  } else {
    for (const replacement of [false, true]) {
      console.log(
        JSON.stringify({
          acceptanceAdapter: replacement ? "replacement" : "default",
        }),
      );
      const page = await browser.newPage(),
        errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") console.error(message.text());
      });
      page.on("websocket", (socket) =>
        socket.on("socketerror", (error) => console.error(error)),
      );
      await page.goto(origin + "/apps/rui/");
      await page.waitForFunction(() => !!window.consumer);
      const address = replacement
        ? `wss://127.0.0.1:${hostPort}/rui`
        : origin.replace("https:", "wss:") + "/rui";
      await page.evaluate(
        async ({ address, credential, replacement }) => {
          await window.consumer.create(
            address,
            credential,
            replacement,
            Array(16).fill(replacement ? 4 : 3),
          );
          await window.consumer.connect();
        },
        {
          address,
          credential: replacement ? secondCredential : credential,
          replacement,
        },
      );
      const inspect = () => page.evaluate(() => window.consumer.inspect());
      await wait(inspect, ready);
      assert.equal((await inspect()).hasControl, false);
      await page.evaluate(() => window.consumer.requestControl());
      await wait(inspect, (s) => ready(s) && s.hasControl);
      const sequence = await page.evaluate(() => window.consumer.activate());
      await wait(
        inspect,
        (s) =>
          ready(s) &&
          s.results.some(
            (r) => r.sequence === sequence && r.status === "Applied",
          ),
      );
      const focus = await page.evaluate(() => window.consumer.focusText());
      await wait(
        inspect,
        (s) =>
          ready(s) &&
          s.results.some((r) => r.sequence === focus && r.status === "Applied"),
      );
      const value = `Installed ${replacement ? "replacement" : "default"} 𝄞 é`;
      const edit = await page.evaluate(
        (value) => window.consumer.edit(value),
        value,
      );
      await wait(
        inspect,
        (s) =>
          ready(s) &&
          s.text === value &&
          s.results.some((r) => r.sequence === edit && r.status === "Applied"),
      );
      const nativeBefore = await newest();
      assert(
        nativeBefore.semantics.some((n) => n.text?.content === value),
        "guarded edit did not reach native source",
      );
      await page.evaluate(() => window.consumer.releaseControl());
      await wait(inspect, (s) => ready(s) && !s.hasControl);
      // Host-side keyboard edit, discovered from native editable semantics/bindings.
      const field = nativeBefore.semantics.find((n) => n.text?.editable);
      assert(field);
      const binding = nativeBefore.surfaces
        .flatMap((s) => s.binding_regions)
        .find((b) => b.semantic_id === field.id);
      const [x, y, w, h] = binding.interaction_region;
      native("click", x + w / 2, y + h / 2);
      native("keys", "+Control_L", "a", "-Control_L", "h", "o", "s", "t");
      await wait(inspect, (s) => ready(s) && s.text === "host");
      await page.evaluate(() => window.consumer.disconnect());
      assert.equal((await inspect()).connection, "disconnected");
      await page.evaluate(() => window.consumer.connect());
      await wait(inspect, (s) => ready(s) && s.text === "host");
      assert.equal(
        (await inspect()).hasControl,
        false,
        "reconnect restored authority",
      );
      const final = await inspect();
      assert.deepEqual(final.failures, []);
      assert.deepEqual(errors, []);
      if (replacement) {
        assert(final.renders > 0);
        assert(final.sends > 0);
      }
      await page.evaluate(() => window.consumer.dispose());
      const disposed = await inspect();
      assert.equal(disposed.connection, "disposed");
      assert.equal(disposed.memoryUsed, 0);
      if (replacement) assert.equal(disposed.releases, 1);
      reports.push({
        transport: replacement
          ? "replacement/direct-WSS"
          : "default/verifying-proxy",
        renderer: replacement ? "replacement" : "CanvasRenderer",
        observed: true,
        controlRequestedAndReleased: true,
        guardedButtonAndUnicodeEdit: true,
        sourceEdit: true,
        explicitReconnect: true,
        reconnectWithoutAuthority: true,
        disposalMemory: disposed.memoryUsed,
        renders: final.renders,
        sends: final.sends,
      });
      await page.close();
    }
    // Final disposal does not reset the host's producer ledger. A recreated actor
    // with a reused approved identity may acquire with its fresh request namespace,
    // but must not dispatch a new edit as the old command number one.
    const before = (await newest()).semantics.map((n) => [
      n.id,
      n.label,
      n.text?.content,
    ]);
    const reused = await browser.newPage();
    await reused.goto(origin + "/apps/rui/");
    await reused.waitForFunction(() => !!window.consumer);
    await reused.evaluate(
      async ({ address, credential }) => {
        await window.consumer.create(address, credential);
        await window.consumer.connect();
      },
      { address: origin.replace("https:", "wss:") + "/rui", credential },
    );
    const reusedState = () => reused.evaluate(() => window.consumer.inspect());
    await wait(reusedState, ready);
    await reused.evaluate(() => window.consumer.requestControl());
    await wait(reusedState, (s) => ready(s) && s.hasControl);
    assert.equal(await reused.evaluate(() => window.consumer.activate()), 1n);
    await wait(reusedState, (s) =>
      s.events.some(
        (e) => e.type === "error" && e.code === "AUTHORITY_REJECTED",
      ),
    );
    assert.equal(
      (await reusedState()).pending,
      1,
      "conflicting producer bytes were not retained",
    );
    assert.equal((await reusedState()).hasControl, false);
    assert.deepEqual(
      (await newest()).semantics.map((n) => [n.id, n.label, n.text?.content]),
      before,
    );
    await reused.evaluate(() => window.consumer.dispose());
    await reused.close();
    const result = {
      ...prepared.report,
      passed: true,
      runtime: "chromium",
      version: browser.version(),
      productionHttps: true,
      strictCsp: true,
      backendCertificateVerified: true,
      reports,
      freshRequestNamespace: true,
      discardedProducerSequenceRejected: true,
      scope:
        "Owned single machine loopback and private GTK display; fixture certificate key exceptions. Two-machine tailnet/trusted-device acceptance not established.",
    };
    await mkdir(resolve(packageRoot, "test-output"), { recursive: true });
    await writeFile(
      resolve(packageRoot, "test-output/package-live.json"),
      JSON.stringify(result, null, 2) + "\n",
    );
    console.log(
      JSON.stringify({
        passed: true,
        installedPackage: true,
        productionHttps: true,
        reports,
      }),
    );
  }
} catch (error) {
  console.error(logs.slice(-3000));
  throw error;
} finally {
  await browser?.close();
  if (browserProcess && browserProcess.exitCode === null) {
    browserProcess.kill("SIGTERM");
    await new Promise((done) => {
      browserProcess.once("exit", done);
      setTimeout(() => {
        browserProcess.kill("SIGKILL");
        done();
      }, 3000).unref();
    });
  }
  if (serving) await new Promise((done) => serving.httpServer.close(done));
  if (host && host.exitCode === null) {
    host.stdin.write("\n");
    await new Promise((done) => {
      host.once("exit", done);
      setTimeout(() => {
        host.kill("SIGKILL");
        done();
      }, 3000).unref();
    });
  }
  await rm(temporary, { recursive: true, force: true });
}
