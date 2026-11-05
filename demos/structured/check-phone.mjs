import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { writeFile } from "node:fs/promises";
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
    viewport: { width: 390, height: 844 },
    deviceScaleFactor: 1,
    isMobile: true,
    hasTouch: true,
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(url);
  await page.selectOption("#workload", "chart");
  await page.selectOption("#width", "320");
  await page.click("#run");
  await page.waitForFunction(
    () => !document.querySelector("#run").disabled,
    {},
    { timeout: 90000 },
  );
  assert.equal(
    await page.locator("#save").isEnabled(),
    true,
    await page.locator("#status").innerText(),
  );
  const result = JSON.parse(await page.locator("#result").innerText());
  assert.equal(result.frames, 300);
  assert.equal(result.width, 320);
  assert.deepEqual(errors, []);
  assert(
    await page.evaluate(() => {
      const c = document.querySelector("canvas");
      const pixels = c
        .getContext("2d")
        .getImageData(0, 0, c.width, c.height).data;
      return [...pixels].some((v, i) => i % 4 !== 3 && v > 150);
    }),
    "final frame was cleared",
  );
  await page.screenshot({
    path: resolve(directory, "mobile-runner.png"),
    fullPage: true,
  });
  await writeFile(
    resolve(directory, "mobile-runner.json"),
    JSON.stringify(
      {
        ...result,
        physicalPhone: false,
        scope:
          "Chromium touch/viewport emulation on the desktop; functional runner check only",
      },
      null,
      2,
    ) + "\n",
  );
  console.log(
    JSON.stringify({
      frames: result.frames,
      physicalPhone: false,
      runnerPassed: true,
    }),
  );
} finally {
  await browser?.close();
  server.kill();
}
