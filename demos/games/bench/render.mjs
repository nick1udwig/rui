import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createReadStream } from "node:fs";
import { readFile, appendFile, writeFile, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { chromium } from "../../../client-browser/node_modules/playwright/index.mjs";
import { cpus, release } from "node:os";
import { createHash } from "node:crypto";
const root = resolve(import.meta.dirname, "../../..");
const directory = resolve(process.argv[2] ?? "");
assert(process.argv[2], "usage: node render.mjs CAPTURE_DIRECTORY");
const capture = JSON.parse(await readFile(resolve(directory, "capture.json")));
const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, "http://localhost").pathname;
    if (pathname === "/") {
      response.setHeader("Content-Type", "text/html");
      response.end('<!doctype html><canvas id="screen"></canvas>');
      return;
    }
    const base = pathname.startsWith("/data/") ? directory : root;
    const path = resolve(
      base,
      "." + (pathname.startsWith("/data/") ? pathname.slice(5) : pathname),
    );
    if (!path.startsWith(base + sep) || !(await stat(path)).isFile())
      throw new Error("path");
    response.setHeader(
      "Content-Type",
      path.endsWith(".wasm")
        ? "application/wasm"
        : path.endsWith(".js") || path.endsWith(".mjs")
          ? "text/javascript"
          : "application/octet-stream",
    );
    response.end(await readFile(path));
  } catch {
    response.writeHead(404);
    response.end();
  }
});
let browser;
try {
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.RUI_WASM_CHROMIUM ?? "/usr/bin/chromium",
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  let activeFile,
    nextFrame = 0,
    frameBytes = 0;
  await page.exposeFunction("saveFrame", async (index, encoded) => {
    assert.equal(index, nextFrame++);
    const bytes = Buffer.from(encoded, "base64");
    assert.equal(bytes.length, frameBytes);
    await appendFile(activeFile, bytes);
  });
  const results = [];
  for (const [width, height] of [
    [320, 240],
    [640, 480],
    [1280, 960],
  ]) {
    activeFile = resolve(directory, `${width}.rgba`);
    nextFrame = 0;
    frameBytes = width * height * 4;
    await writeFile(activeFile, "");
    const measured = await page.evaluate(
      async ({ width, height }) => {
        const { default: init, Receiver } =
          await import("/client-browser/wasm/rui_client_wasm.js");
        const { CanvasRenderer } =
          await import("/client-browser/dist/index.js");
        await init();
        const bytes = new Uint8Array(
          await (await fetch("/data/publications.rui")).arrayBuffer(),
        );
        const frames = [];
        let offset = 0;
        while (offset < bytes.length) {
          const length = new DataView(bytes.buffer, offset, 4).getUint32(0);
          offset += 4;
          if (offset + length > bytes.length)
            throw new Error("truncated publication");
          frames.push(bytes.slice(offset, offset + length));
          offset += length;
        }

        const runs = [];
        for (let run = 0; run < 4; run++) {
          const canvas = document.createElement("canvas");
          document.body.replaceChildren(canvas);
          const receiver = new Receiver(
            new Uint8Array(16).fill(1),
            new Uint8Array(16).fill(2),
            new Uint8Array(16).fill(3),
            new Uint8Array(16).fill(4),
            64 * 1024 * 1024,
            true,
          );
          const renderer = new CanvasRenderer(canvas);
          const samples = [];
          for (let i = 0; i < frames.length; i++) {
            const publication = new Uint8Array(16);
            new DataView(publication.buffer).setUint32(12, i + 1);
            const start = performance.now();
            receiver.receiveEnvelope(publication, frames[i]);
            const reduced = performance.now();
            if (receiver.revision !== BigInt(i + 1))
              throw new Error("replay revision mismatch");
            const state = receiver.state();
            const exported = performance.now();
            renderer.render(
              state,
              { width, height, origin: { x: 0, y: 0 }, scale: width / 640 },
              () => {
                throw new Error("unexpected resource");
              },
            );
            const painted = performance.now();
            if (run > 0)
              samples.push({
                frame: i,
                reduceMs: reduced - start,
                exportMs: exported - reduced,
                drawMs: painted - exported,
                totalMs: painted - start,
              });
            if (run === 0) {
              const data = canvas
                .getContext("2d")
                .getImageData(0, 0, width, height).data;
              let string = "";
              for (let k = 0; k < data.length; k += 16384)
                string += String.fromCharCode(...data.subarray(k, k + 16384));
              await window.saveFrame(i, btoa(string));
            }
          }
          if (run > 0) runs.push(samples);
          receiver.close();
          receiver.free();
          renderer.dispose();
        }
        return { width, height, frames: frames.length, runs };
      },
      { width, height },
    );
    assert.equal(nextFrame, capture.frames);
    const file = await stat(activeFile);
    assert.equal(file.size, capture.frames * frameBytes);
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(activeFile)) hash.update(chunk);
    measured.rgbaSha256 = hash.digest("hex");
    results.push(measured);
    console.log(JSON.stringify({ width, height, frames: measured.frames }));
  }
  const hashes = {};
  for (const name of [
    "client-browser/dist/renderer.js",
    "client-browser/wasm/rui_client_wasm_bg.wasm",
    "browser-host/src/projection.rs",
    "browser-host/src/bin/game-trace.rs",
  ])
    hashes[name] = createHash("sha256")
      .update(await readFile(resolve(root, name)))
      .digest("hex");
  await writeFile(
    resolve(directory, "render.json"),
    JSON.stringify(
      {
        browser: browser.version(),
        cpu: cpus()[0].model,
        kernel: release(),
        node: process.version,
        hashes,
        results,
      },
      null,
      2,
    ) + "\n",
  );
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
