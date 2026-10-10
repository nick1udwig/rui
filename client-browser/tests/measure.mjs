import { createServer } from "node:http";
import { readFile, stat, mkdir, writeFile } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { createHash } from "node:crypto";
import { cpus, platform, release } from "node:os";
import { chromium, firefox, webkit } from "playwright";
const root = resolve(import.meta.dirname, "..");
const output = process.env.RUI_RENDER_OUTPUT;
if (!output)
  throw new Error("RUI_RENDER_OUTPUT must name a new evidence directory");
await mkdir(output, { recursive: false });
const sourceHashes = Object.fromEntries(
  await Promise.all(
    ["src/renderer.ts", "dist/renderer.js", "tests/benchmark.mjs"].map(
      async (name) => [
        name,
        createHash("sha256")
          .update(await readFile(resolve(root, name)))
          .digest("hex"),
      ],
    ),
  ),
);
const server = createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.writeHead(200, { "Content-Type": "text/html" });
      res.end("<!doctype html><title>RUI renderer benchmark</title>");
      return;
    }
    const path = resolve(
      root,
      "." + new URL(req.url, "http://localhost").pathname,
    );
    if (!path.startsWith(root + sep) || !(await stat(path)).isFile())
      throw new Error();
    res.writeHead(200, { "Content-Type": "text/javascript" });
    res.end(await readFile(path));
  } catch {
    res.writeHead(404);
    res.end();
  }
});
let browser;
try {
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  for (const name of (process.env.RUI_TEST_BROWSERS ?? "chromium").split(",")) {
    const engine = { chromium, firefox, webkit }[name];
    if (!engine) throw new Error("invalid browser");
    browser = await engine.launch({
      headless: true,
      executablePath: process.env[`RUI_WASM_${name.toUpperCase()}`],
    });
    for (let run = 1; run <= 3; run++) {
      const page = await browser.newPage();
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      const measured = await page.evaluate(async () =>
        (await import("/tests/benchmark.mjs")).measure(),
      );
      const report = {
        browser: name,
        version: browser.version(),
        hardware: cpus()[0].model,
        kernel: `${platform()} ${release()}`,
        node: process.version,
        sourceHashes,
        run,
        ...measured,
      };
      await writeFile(
        resolve(output, `${name}-${run}.json`),
        JSON.stringify(report) + "\n",
      );
      const sorted = measured.samples.toSorted((a, b) => a - b);
      console.log(
        JSON.stringify({
          browser: name,
          run,
          p50: sorted[299],
          p95: sorted[569],
          p99: sorted[593],
          ownership: measured.ownership,
        }),
      );
      await page.close();
    }
    await browser.close();
    browser = undefined;
  }
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
