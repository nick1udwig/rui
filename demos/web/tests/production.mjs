import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { resolve, sep, extname } from "node:path";
import {
  chromium,
  firefox,
  webkit,
} from "../../../client-browser/node_modules/playwright/index.mjs";

const root = resolve(import.meta.dirname, "../dist");
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://localhost");
    const path = resolve(
      root,
      "." + (url.pathname === "/" ? "/index.html" : url.pathname),
    );
    if (
      !path.startsWith(root + sep) ||
      (await stat(path)).size > 4 * 1024 * 1024
    )
      throw new Error();
    const mime = {
      ".html": "text/html",
      ".js": "text/javascript",
      ".css": "text/css",
      ".wasm": "application/wasm",
    };
    response.writeHead(200, {
      "Content-Type": mime[extname(path)] ?? "application/octet-stream",
    });
    response.end(await readFile(path));
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
  const engines = { chromium, firefox, webkit };
  for (const name of (process.env.RUI_TEST_BROWSERS ?? "chromium").split(",")) {
    if (!engines[name]) throw new Error("unsupported test browser");
    browser = await engines[name].launch({
      headless: true,
      executablePath: process.env[`RUI_WASM_${name.toUpperCase()}`],
    });
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    await page
      .locator("#credential")
      .setInputFiles({
        name: "fixture.credential",
        mimeType: "application/octet-stream",
        buffer: Buffer.alloc(32, 7),
      });
    const wasmResponse = page.waitForResponse((r) =>
      new URL(r.url()).pathname.endsWith(".wasm"),
    );
    await page.locator("#connect").click();
    const wasm = await wasmResponse;
    assert.equal(wasm.headers()["content-type"], "application/wasm");
    const module = await WebAssembly.compile(await wasm.body());
    assert(
      WebAssembly.Module.imports(module).every(
        (i) => i.module !== "wasi_snapshot_preview1",
      ),
    );
    // There is deliberately no host in this asset check. Successful WASM init
    // reaches transport setup and its bounded failure path.
    await page
      .locator("#status")
      .filter({
        hasText: /WebSocket connection failed|host closed the connection/,
      })
      .waitFor();
    assert.deepEqual(errors, []);
    console.log(
      JSON.stringify({
        runtime: name,
        version: browser.version(),
        productionWasm: true,
        transportFailure: true,
      }),
    );
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
