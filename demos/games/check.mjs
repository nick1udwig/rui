import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import { chromium } from "../../client-browser/node_modules/playwright/index.mjs";
assert.equal(process.env.RUI_ISOLATED_DISPLAY, "1");
const temporary = await mkdtemp(join(tmpdir(), "rui-game-check-"));
const state = join(temporary, "state");
let logs = "",
  browser;
const launcher = spawn(
  "python3",
  [
    "demos/games/run.py",
    "--skip-build",
    "--game",
    process.env.RUI_GAME ?? "asteroid",
    "--mame",
    process.env.RUI_MAME,
    "--rompath",
    process.env.RUI_ROMPATH,
    "--plugins",
    process.env.RUI_MAME_PLUGINS,
    "--state",
    state,
    "--public-host",
    "127.0.0.1",
  ],
  { cwd: resolve(import.meta.dirname, "../.."), env: process.env },
);
launcher.stdout.on("data", (bytes) => {
  logs = (logs + bytes).slice(-16000);
});
launcher.stderr.on("data", (bytes) => {
  logs = (logs + bytes).slice(-16000);
});
async function wait(read, condition = Boolean, ms = 20000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    try {
      const value = await read();
      if (condition(value)) return value;
    } catch {}
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  let native = "";
  try {
    native = await readFile(join(state, "mame.log"), "utf8");
  } catch {}
  throw new Error("state did not converge\n" + logs + "\n" + native);
}
const native = () =>
  readFile(join(state, "mame/frame.json"), "utf8").then(JSON.parse);
try {
  await wait(async () => (await fetch("http://127.0.0.1:5173")).ok);
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.RUI_WASM_CHROMIUM ?? "/usr/bin/chromium",
    args: ["--no-sandbox"],
  });
  const page = await browser.newPage({
    viewport: { width: 1100, height: 1050 },
  });
  page.setDefaultTimeout(5000);
  page.on("pageerror", (error) => {
    logs += "\nPAGE " + error;
  });
  page.on("websocket", (socket) =>
    socket.on("close", () => {
      logs += "\nWS closed";
    }),
  );
  page.on("console", (message) => {
    if (message.type() === "error") logs += "\nCONSOLE " + message.text();
  });
  await page.goto("http://127.0.0.1:5173");
  await page
    .locator("#credential")
    .setInputFiles(join(state, "client.credential"));
  await page.locator("#connect").click();
  await wait(() => page.locator("#control").isEnabled());
  await page.locator("#control").click();
  await wait(() => page.evaluate(() => window.ruiGame().controller));
  await wait(native, (frame) => frame.lines.length > 40);
  assert(
    (await native()).lines.some((line) => Math.max(...line.slice(0, 4)) > 100),
    "native beam coordinate scale",
  );
  for (const [label, bit] of [
    ["Insert coin", 32],
    ["Start", 64],
  ]) {
    await page.getByRole("button", { name: label, exact: true }).hover();
    await page.mouse.down();
    await new Promise((resolve) => setTimeout(resolve, 300));
    console.error(
      "INPUT",
      await page.locator("#status").textContent(),
      await page.evaluate(() => window.ruiGame()),
    );
    await wait(native, (frame) => (frame.mask & bit) !== 0);
    await new Promise((resolve) => setTimeout(resolve, 180));
    await page.mouse.up();
    await wait(native, (frame) => frame.mask === 0);
  }
  await page.locator("h1").click();
  await page.keyboard.down("Space");
  await wait(native, (frame) => (frame.mask & 4) !== 0);
  await page.keyboard.down("ArrowLeft");
  await wait(native, (frame) => (frame.mask & 5) === 5);
  await page.keyboard.up("ArrowLeft");
  await page.keyboard.up("Space");
  await wait(native, (frame) => frame.mask === 0);
  const before = await page.evaluate(() => window.ruiGame());
  await new Promise((resolve) => setTimeout(resolve, 6000));
  const after = await page.evaluate(() => window.ruiGame());
  const fps = (after.presentedFrames - before.presentedFrames) / 6;
  assert(fps >= 15, `game frame rate ${fps} < 15`);
  assert(after.inputTimes.length >= 8);
  await page.selectOption("#zoom", "1.5");
  await wait(
    () => page.locator("canvas").getAttribute("width"),
    (width) => width === "960",
  );
  const litPixels = await page.locator("canvas").evaluate((canvas) => {
    const ctx = canvas.getContext("2d");
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    let count = 0;
    for (let i = 0; i < data.length; i += 4)
      if (data[i] + data[i + 1] + data[i + 2] > 40) count++;
    return count;
  });
  assert(litPixels > 2000, `visible vector coverage ${litPixels}`);
  await page.screenshot({
    path: process.env.RUI_GAME_SCREENSHOT ?? "/tmp/rui-game-proof.png",
    fullPage: true,
  });
  await page.locator("h1").click();
  await page.keyboard.down("Space");
  await wait(native, (frame) => (frame.mask & 4) !== 0);
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await wait(native, (frame) => frame.mask === 0);
  await wait(() => page.evaluate(() => !window.ruiGame().controller));
  await page.keyboard.up("Space");
  await page.locator("#control").click();
  await wait(() => page.evaluate(() => window.ruiGame().controller));
  await page.selectOption("#zoom", "0.75");
  await page.locator("h1").click();
  await page.keyboard.down("Space");
  await wait(native, (frame) => (frame.mask & 4) !== 0);
  await page.close();
  await wait(native, (frame) => frame.mask === 0);
  const times = after.inputTimes.sort((a, b) => a - b);
  const report = {
    game: process.env.RUI_GAME ?? "asteroid",
    fps,
    inputAckP95Ms: times[Math.floor(times.length * 0.95)],
    samples: times.length,
    maxLines: (await native()).lines.length,
    litPixels,
    nativeInputs: true,
    releaseOnBlur: true,
    releaseOnDisconnect: true,
    receivedKiBPerSecond:
      (after.receivedBytes - before.receivedBytes) / 1024 / 6,
  };
  if (process.env.RUI_GAME_REPORT)
    await writeFile(
      process.env.RUI_GAME_REPORT,
      JSON.stringify(report, null, 2) + "\n",
    );
  console.log(JSON.stringify(report));
} catch (error) {
  console.error(logs);
  throw error;
} finally {
  await browser?.close();
  launcher.kill("SIGTERM");
  await new Promise((resolve) => launcher.once("exit", resolve));
  if (process.env.RUI_GAME_KEEP) console.error("Retained state: " + state);
  else await rm(temporary, { recursive: true, force: true });
}
