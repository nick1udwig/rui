import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, firefox, webkit } from "playwright";
import { initSync } from "../pkg/rui_client_wasm.js";
import { runChecks } from "../test-dist/smoke.js";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const fixtures = JSON.parse(
  await readFile(resolve(root, "pkg/fixtures.json"), "utf8"),
);
const bytes = await readFile(resolve(root, "pkg/rui_client_wasm_bg.wasm"));
const module = new WebAssembly.Module(bytes);
assert(
  WebAssembly.Module.imports(module).every(
    (entry) => entry.module !== "wasi_snapshot_preview1",
  ),
);
initSync({ module });
console.log(
  JSON.stringify({ runtime: "node", ...(await runChecks(fixtures)) }),
);

const server = createServer(async (request, response) => {
  try {
    if (request.url === "/") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end("<!doctype html><title>RUI WASM feasibility</title>");
      return;
    }
    const pathname = decodeURIComponent(
      new URL(request.url, "http://localhost").pathname,
    );
    const path = resolve(root, "." + pathname);
    if (!path.startsWith(root + sep) || !(await stat(path)).isFile()) {
      response.writeHead(404);
      response.end();
      return;
    }
    response.writeHead(200, {
      "Content-Type": path.endsWith(".wasm")
        ? "application/wasm"
        : "text/javascript",
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
  const selected = (process.env.RUI_TEST_BROWSERS ?? "chromium").split(",");
  if (!selected.length || selected.some((name) => !engines[name]))
    throw new Error("unsupported test browser");
  for (const name of selected) {
    browser = await engines[name].launch({
      headless: true,
      executablePath: process.env[`RUI_WASM_${name.toUpperCase()}`],
    });
    const page = await browser.newPage();
    page.setDefaultTimeout(15000);
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    const result = await page.evaluate(async (input) => {
      const { default: init } = await import("/pkg/rui_client_wasm.js");
      await init(); // Real browser fetch/instantiate of the same ES module build.
      const { runChecks: check } = await import("/test-dist/smoke.js");
      return check(input);
    }, fixtures);
    console.log(
      JSON.stringify({ runtime: name, version: browser.version(), ...result }),
    );
    await browser.close();
    browser = undefined;
  }
} finally {
  if (browser) await browser.close();
  await new Promise((done) => server.close(done));
}
