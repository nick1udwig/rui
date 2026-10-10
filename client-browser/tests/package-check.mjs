import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, stat, mkdir, rm, writeFile } from "node:fs/promises";
import { resolve, sep, extname } from "node:path";
import { chromium, firefox, webkit } from "playwright";
import { prepareConsumer, packageRoot } from "./package-support.mjs";

const prepared = await prepareConsumer();
const root = resolve(prepared.consumer, "dist"),
  base = "/apps/rui/";
const csp =
  "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; style-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'";
let denyWasm = false;
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://localhost");
    if (url.pathname === base + "corrupt.wasm") {
      response.writeHead(200, { "Content-Type": "application/wasm" });
      response.end("invalid ABI");
      return;
    }
    if (url.pathname === base + "foreign.wasm") {
      response.writeHead(200, { "Content-Type": "application/wasm" });
      response.end(Buffer.from([0, 97, 115, 109, 1, 0, 0, 0]));
      return;
    }
    if (url.pathname === base + "oversized.wasm") {
      response.writeHead(200, { "Content-Type": "application/wasm" });
      response.end(Buffer.alloc(8 * 1024 * 1024 + 1));
      return;
    }
    let pathname = url.pathname;
    const wrongMime = pathname === base + "wrong-mime.wasm";
    if (wrongMime) pathname = base + "assets/" + prepared.wasm;
    if (!pathname.startsWith(base)) throw Error();
    const file = resolve(root, pathname.slice(base.length) || "index.html");
    if (!file.startsWith(root + sep) || !(await stat(file)).isFile())
      throw Error();
    const mime = {
      ".html": "text/html",
      ".js": "text/javascript",
      ".wasm": "application/wasm",
    };
    response.writeHead(200, {
      "Content-Type": wrongMime
        ? "application/octet-stream"
        : (mime[extname(file)] ?? "application/octet-stream"),
      "Content-Security-Policy": denyWasm
        ? csp.replace(" 'wasm-unsafe-eval'", "")
        : csp,
    });
    response.end(await readFile(file));
  } catch {
    response.writeHead(404);
    response.end();
  }
});
let browser;
try {
  await new Promise((done, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", done);
  });
  const origin = `http://127.0.0.1:${server.address().port}`;
  const reports = [];
  const engines = { chromium, firefox, webkit };
  for (const name of (process.env.RUI_TEST_BROWSERS ?? "chromium").split(",")) {
    assert(engines[name]);
    browser = await engines[name].launch({
      headless: true,
      executablePath: process.env[`RUI_WASM_${name.toUpperCase()}`],
    });
    const page = await browser.newPage(),
      errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(origin + base);
    await page.waitForFunction(() => !!window.consumer);
    assert.equal(
      await page.evaluate(() =>
        window.consumer.initialize("/apps/rui/missing.wasm"),
      ),
      false,
    );
    assert.equal(
      await page.evaluate(() =>
        window.consumer.initialize("/apps/rui/corrupt.wasm"),
      ),
      false,
    );
    assert.equal(
      await page.evaluate(() =>
        window.consumer.initialize("/apps/rui/foreign.wasm"),
      ),
      false,
    );
    assert.equal(
      await page.evaluate(() =>
        window.consumer.initialize("/apps/rui/oversized.wasm"),
      ),
      false,
    );
    const response = page.waitForResponse((r) =>
      r.url().endsWith(prepared.wasm),
    );
    assert.deepEqual(
      await page.evaluate(() =>
        Promise.all([
          window.consumer.initialize(),
          window.consumer.initialize(),
        ]),
      ),
      [true, true],
    );
    const wasm = await response;
    assert.equal(wasm.headers()["content-type"], "application/wasm");
    assert.equal(
      await page.evaluate(() =>
        window.consumer.initialize("/apps/rui/corrupt.wasm"),
      ),
      false,
      "loaded ABI cannot be replaced",
    );
    for (const replacement of [false, true]) {
      await page.evaluate(
        async ({ url, replacement }) => {
          await window.consumer.create(url, Array(32).fill(7), replacement);
          try {
            await window.consumer.connect();
          } catch {
            /* expected absent host */
          }
          window.consumer.dispose();
        },
        { url: origin.replace("http:", "ws:") + "/rui", replacement },
      );
      const result = await page.evaluate(() => window.consumer.inspect());
      assert.equal(result.connection, "disposed");
      assert.equal(result.memoryUsed, 0);
      assert.equal(result.pending, 0);
      assert.equal(result.resourcesReady, false);
      assert.equal(result.hasControl, false);
      assert.deepEqual(result.capabilities.profiles, []);
      assert(result.failures.length > 0);
      if (replacement) assert.equal(result.releases, 1);
    }
    assert.deepEqual(errors, []);
    // Each new page has a fresh module instance, so failures/retries are real.
    await page.close();
    const mimePage = await browser.newPage();
    await mimePage.goto(origin + base);
    await mimePage.waitForFunction(() => !!window.consumer);
    assert.equal(
      await mimePage.evaluate(() =>
        window.consumer.initialize("/apps/rui/wrong-mime.wasm"),
      ),
      true,
    );
    await mimePage.close();
    denyWasm = true;
    const blocked = await browser.newPage();
    await blocked.goto(origin + base);
    await blocked.waitForFunction(() => !!window.consumer);
    assert.equal(
      await blocked.evaluate(() => window.consumer.initialize()),
      false,
    );
    denyWasm = false;
    await blocked.close();
    reports.push({
      runtime: name,
      version: browser.version(),
      productionWasm: true,
      nonRootAssets: true,
      initialization404AndCorruption: true,
      foreignAbiAndAssetLimit: true,
      retryAndConcurrentInit: true,
      immutableAbi: true,
      mimeFallback: true,
      cspEnforced: true,
      defaultAndReplacementAdapters: true,
      disposal: true,
      scope:
        "distributed assets and absent-host failure paths; live source gate is separate",
    });
    await browser.close();
    browser = undefined;
  }
  const result = { ...prepared.report, reports };
  await mkdir(resolve(packageRoot, "test-output"), { recursive: true });
  await writeFile(
    resolve(packageRoot, "test-output/package.json"),
    JSON.stringify(result, null, 2) + "\n",
  );
  console.log(JSON.stringify({ distributedPackage: true, reports }));
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
  await rm(prepared.temporary, { recursive: true, force: true });
}
