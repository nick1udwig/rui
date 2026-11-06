import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { open, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { chromium } from "../../client-browser/node_modules/playwright/index.mjs";
const directory = resolve(process.argv[2]);
let browser;
const server = spawn(
  "python3",
  ["demos/structured/serve.py", "--data", directory, "--port", "0"],
  { stdio: ["ignore", "pipe", "inherit"] },
);
try {
  const url = await new Promise((resolve, reject) => {
    server.stdout.once("data", (b) =>
      resolve(b.toString().trim().split(" ").at(-1)),
    );
    server.once("error", reject);
    server.once("exit", () => reject(Error("server exited")));
  });
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.RUI_WASM_CHROMIUM ?? "/usr/bin/chromium",
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage({
    viewport: { width: 1000, height: 1000 },
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(url + "demos/structured/live.html");
  const reports = [];
  for (const variant of ["objects", "flattened"]) {
    const count = JSON.parse(
      await readFile(resolve(directory, variant, "capture.json")),
    ).frames;
    await page.selectOption("#variant", variant);
    await page.click("#play");
    await page.waitForFunction(
      () => Number(document.querySelector("#frames").textContent) >= 20,
      {},
      { timeout: 30000 },
    );
    await page.click("#pause");
    await page.waitForTimeout(300);
    const frozen = await page.locator("#bytes").getAttribute("data-bytes");
    await page.selectOption("#width", "320");
    await page.waitForTimeout(200);
    assert.equal(
      await page.locator("#bytes").getAttribute("data-bytes"),
      frozen,
      "zoom requested more host data while paused",
    );
    await page.selectOption("#width", "640");
    await page.click("#pause");
    await page.waitForFunction(
      () =>
        document
          .querySelector("#status")
          .textContent.startsWith("Replay complete"),
      {},
      { timeout: 60000 },
    );
    assert.equal(await page.locator("#frames").innerText(), String(count));
    const data = await readFile(
      resolve(directory, variant, "publications.packets"),
    );
    let bytes = 0;
    for (let i = 0; i < data.length;) {
      const n = data.readUInt32BE(i);
      i += 4 + n;
      bytes += n;
      assert(i <= data.length);
    }
    assert.equal(
      Number(await page.locator("#bytes").getAttribute("data-bytes")),
      bytes,
    );
    const actual = await page.evaluate(async () => {
      const c = document.querySelector("canvas");
      const p = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
      return [...new Uint8Array(await crypto.subtle.digest("SHA-256", p))]
        .map((x) => x.toString(16).padStart(2, "0"))
        .join("");
    });
    const raw = await open(resolve(directory, "objects", "640.rgba"));
    const last = Buffer.alloc(640 * 480 * 4);
    await raw.read(last, 0, last.length, last.length * (count - 1));
    await raw.close();
    assert.equal(actual, createHash("sha256").update(last).digest("hex"));
    await page.screenshot({
      path: resolve(directory, variant + "-live.png"),
      fullPage: true,
    });
    reports.push({
      variant,
      frames: count,
      applicationBytes: bytes,
      lastFrameSha256: actual,
      pauseBackpressure: true,
      localZoomWithoutTraffic: true,
    });
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({
    path: resolve(directory, "live-mobile-layout.png"),
    fullPage: true,
  });
  assert.deepEqual(errors, []);
  await writeFile(
    resolve(directory, "live-check.json"),
    JSON.stringify(
      {
        reports,
        physicalPhone: false,
        scope:
          "Actual browser WebSocket replay with real rendering and presentation acknowledgements; desktop Chromium, including a narrow layout check",
      },
      null,
      2,
    ) + "\n",
  );
  console.log(JSON.stringify(reports));
} finally {
  await browser?.close();
  server.kill();
}
