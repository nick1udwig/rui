import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { chromium, firefox, webkit } from "playwright";
import { checkLayout } from "./layout.mjs";
const root = resolve(import.meta.dirname, "..");
const server = createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.writeHead(200, { "Content-Type": "text/html" });
      res.end("<!doctype html><title>RUI browser conformance</title>");
      return;
    }
    if (req.url === "/semantic-demo.css") {
      res.writeHead(200, { "Content-Type": "text/css" });
      res.end(await readFile(resolve(root, "../demos/semantic/src/style.css")));
      return;
    }
    const path = resolve(
      root,
      "." + new URL(req.url, "http://localhost").pathname,
    );
    if (!path.startsWith(root + sep) || !(await stat(path)).isFile())
      throw new Error();
    res.writeHead(200, {
      "Content-Type": path.endsWith(".wasm")
        ? "application/wasm"
        : "text/javascript",
    });
    res.end(await readFile(path));
  } catch {
    res.writeHead(404);
    res.end();
  }
});
let browser;
try {
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
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
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    const result = await page.evaluate(async () => {
      const { checkPixels } = await import("/tests/pixels.mjs");
      const { measure } = await import("/tests/benchmark.mjs");
      const { checkContract } = await import("/tests/contract.mjs");
      const { checkSemanticView } = await import("/tests/semantic.mjs");
      const { checkSemanticControls } =
        await import("/tests/semantic-controls.mjs");
      return {
        ...checkPixels(),
        ...measure(false),
        ...(await checkContract()),
        ...(await checkSemanticView()),
        ...(await checkSemanticControls()),
      };
    });
    console.log(
      JSON.stringify({
        runtime: name,
        version: browser.version(),
        ...result,
        ...(await checkLayout(page, name, browser.version())),
      }),
    );
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
