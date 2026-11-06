import assert from "node:assert/strict";
import { createServer } from "node:http";
import { readFile, writeFile, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { chromium } from "../../../client-browser/node_modules/playwright/index.mjs";
const directory = resolve(process.argv[2] ?? "");
assert(process.argv[2], "usage: node decode.mjs CAPTURE_DIRECTORY");
const server = createServer(async (request, response) => {
  try {
    if (request.url === "/") {
      response.setHeader("Content-Type", "text/html");
      response.end("<!doctype html><canvas></canvas>");
      return;
    }
    const path = resolve(
      directory,
      "." + new URL(request.url, "http://localhost").pathname,
    );
    if (!path.startsWith(directory + sep) || !(await stat(path)).isFile())
      throw new Error("path");
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
  const results = [];
  for (const [width, height] of [
    [320, 240],
    [640, 480],
    [1280, 960],
  ]) {
    for (const codec of [
      "png",
      "h264-crf18",
      "h264-crf28",
      "h264-crf18-veryfast",
    ]) {
      const name = `${width}-${codec}${codec === "png" ? ".pngstream" : ".h264"}`;
      const measured = await page.evaluate(
        async ({ name, width, height, codec }) => {
          const [body, metadata] = await Promise.all([
            fetch("/" + name).then((r) => r.arrayBuffer()),
            fetch("/" + name + ".json").then((r) => r.json()),
          ]);
          const runs = [];
          const config = {
            codec: metadata.codec,
            hardwareAcceleration: "prefer-software",
            optimizeForLatency: true,
          };
          if (codec !== "png") {
            if (typeof VideoDecoder === "undefined")
              return { unsupported: "VideoDecoder unavailable" };
            if (!(await VideoDecoder.isConfigSupported(config)).supported)
              return { unsupported: JSON.stringify(config) };
          }
          for (let run = 0; run < 3; run++) {
            const canvas = document.createElement("canvas");
            canvas.width = width;
            canvas.height = height;
            document.body.replaceChildren(canvas);
            const context = canvas.getContext("2d");
            const samples = [];
            let pending, decoder;
            if (codec !== "png") {
              decoder = new VideoDecoder({
                output(frame) {
                  const resolve = pending;
                  pending = undefined;
                  resolve(frame);
                },
                error(error) {
                  pending?.(error);
                  pending = undefined;
                },
              });
              decoder.configure(config);
            }
            for (let i = 0; i < metadata.packets.length; i++) {
              const packet = metadata.packets[i];
              const bytes = new Uint8Array(body, packet.pos, packet.size);
              const start = performance.now();
              if (codec === "png") {
                const bitmap = await createImageBitmap(
                  new Blob([bytes], { type: "image/png" }),
                );
                context.drawImage(bitmap, 0, 0);
                bitmap.close();
              } else {
                const framePromise = new Promise((resolve) => {
                  pending = resolve;
                });
                decoder.decode(
                  new EncodedVideoChunk({
                    type: packet.key ? "key" : "delta",
                    timestamp: Math.round((i * 1000000) / metadata.fps),
                    data: bytes,
                  }),
                );
                let timer;
                const frame = await Promise.race([
                  framePromise,
                  new Promise((_, reject) => {
                    timer = setTimeout(
                      () =>
                        reject(new Error("decoder stalled without B-frames")),
                      2000,
                    );
                  }),
                ]);
                clearTimeout(timer);
                if (frame instanceof Error || !frame)
                  throw new Error(String(frame));
                context.drawImage(frame, 0, 0);
                frame.close();
              }
              samples.push({
                frame: i,
                decodeAndBlitMs: performance.now() - start,
              });
            }
            if (decoder) {
              await decoder.flush();
              decoder.close();
            }
            runs.push(samples);
          }
          return {
            runs,
            config: codec === "png" ? "createImageBitmap + drawImage" : config,
          };
        },
        { name, width, height, codec },
      );
      results.push({ width, height, codec, ...measured });
      console.log(
        JSON.stringify({ width, codec, unsupported: measured.unsupported }),
      );
    }
  }
  await writeFile(
    resolve(directory, "decode.json"),
    JSON.stringify({ browser: browser.version(), results }, null, 2) + "\n",
  );
} finally {
  await browser?.close();
  await new Promise((done) => server.close(done));
}
