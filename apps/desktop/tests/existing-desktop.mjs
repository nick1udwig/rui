// Explicit live-desktop smoke: owned Writer/Files instances and temporary data.
import {
  chromium,
  webkit,
} from "../../browser/node_modules/playwright/index.mjs";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, rm, readFile, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import assert from "node:assert/strict";
import { native } from "../workspace.mjs";
import { reverseProxy } from "../../browser/tests/reverse-proxy.mjs";

assert.ok(
  process.env.WAYLAND_DISPLAY,
  "Run explicitly from the host Wayland desktop",
);
const root = resolve(import.meta.dirname, "../../..");
const files = process.env.RUI_REAL_APP === "files";
const appKind = files ? "files" : "writer";
const directory = await mkdtemp(join(tmpdir(), `rui-existing-${appKind}-`));
const folder = join(directory, "Files test");
const documentPath = join(directory, "verification.txt");
const original = "Existing Writer document before RUI";
const edited = "Edited through the mobile client — 東京 👋";
const engine = process.env.RUI_CLIENT_ENGINE || "chromium";
let source,
  host,
  browser,
  page,
  proxy,
  logs = "";
const deadline = async (fn, label, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const result = await fn();
    if (result) return result;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw Error(label);
};
const port = async () => {
  const s = createServer();
  await new Promise((r) => s.listen(0, "127.0.0.1", r));
  const value = s.address().port;
  await new Promise((r) => s.close(r));
  return value;
};
const ownApp = async () => {
  for (const app of (await native({ op: "list" })).apps) {
    const stat = await readFile(`/proc/${app.pid}/stat`, "utf8").catch(
      () => "",
    );
    if (
      Number(
        stat
          .slice(stat.lastIndexOf(")") + 1)
          .trim()
          .split(/\s+/)[2],
      ) === source.pid
    )
      return app;
  }
};
let lastObservationError;
const observe = async (app) => {
  try {
    return await native({ op: "capture", source: app });
  } catch (error) {
    lastObservationError = String(error);
    return { nodes: [] };
  }
};
const paragraph = async (value) => {
  for (const field of await page.locator("textarea").all())
    if ((await field.inputValue()) === value) return field;
};
try {
  await writeFile(documentPath, original + "\n");
  if (files) {
    await mkdir(join(folder, "Example folder"), { recursive: true });
    await writeFile(
      join(folder, "example.txt"),
      "Temporary file for semantic selection\n",
    );
    await writeFile(
      join(folder, "long-" + "x".repeat(170) + ".txt"),
      "Long filename layout check\n",
    );
  }
  source = spawn(
    files ? "/usr/bin/python3" : "/usr/bin/libreoffice",
    files
      ? [join(root, "apps/desktop/tests/files_source.py"), folder]
      : [
          `-env:UserInstallation=${pathToFileURL(join(directory, "profile")).href}`,
          "--writer",
          "--norestore",
          "--nodefault",
          "--nofirststartwizard",
          documentPath,
        ],
    {
      detached: true,
      stdio: ["ignore", "ignore", "ignore"],
      env: {
        ...process.env,
        LANG: "C.UTF-8",
        LC_ALL: "C.UTF-8",
        GDK_BACKEND: "wayland",
        SAL_USE_VCLPLUGIN: "gtk3",
        SAL_ACCESSIBILITY_ENABLED: "1",
        XDG_CONFIG_HOME: join(directory, "config"),
        XDG_DATA_HOME: join(directory, "data"),
      },
    },
  );
  let app = await deadline(ownApp, "existing app accessibility registration");
  // First-run Writer publishes its accessibility root before loading the UI.
  await new Promise((r) => setTimeout(r, 3000));
  // Wait for the real app to finish loading before sharing it.
  await deadline(async () => {
    // Writer may replace its first-run process. Choose the loaded instance
    // before local approval; an existing approval must never follow a restart.
    try {
      app = await ownApp();
      return (
        app &&
        (await observe(app)).nodes.some((n) =>
          files ? n.label === "example.txt. File" : n.value === original,
        )
      );
    } catch {
      return false;
    }
  }, "existing app content loaded");
  const cert = join(directory, "cert.pem"),
    key = join(directory, "key.pem");
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      "1",
      "-subj",
      "/CN=localhost",
      "-addext",
      "subjectAltName=DNS:localhost",
      "-keyout",
      key,
      "-out",
      cert,
    ],
    { stdio: "ignore" },
  );
  const external = await port(),
    backend = await port(),
    origin = `https://localhost:${external}`,
    mount = "/desktop-check";
  proxy = await reverseProxy({
    cert,
    key,
    port: external,
    backendPort: backend,
    mount,
  });
  host = spawn(
    "python3",
    [
      "apps/desktop/run.py",
      "--skip-build",
      "--public-url",
      origin + mount,
      "--port",
      String(backend),
      "--state-dir",
      join(directory, "state"),
    ],
    { cwd: root, stdio: ["pipe", "pipe", "pipe"] },
  );
  host.stdout.on("data", (d) => (logs += d));
  host.stderr.on("data", (d) => (logs += d));
  const invitation = await deadline(
    () => /Open on your phone: (https:\/\/[^\s]+)/.exec(logs)?.[1],
    "host startup",
    40000,
  );
  browser =
    engine === "webkit"
      ? await webkit.launch({
          headless: true,
          executablePath: process.env.RUI_WEBKIT_EXECUTABLE,
        })
      : await chromium.launch({
          executablePath: "/usr/bin/chromium",
          headless: true,
          args: ["--no-sandbox"],
        });
  const context = await browser.newContext({
    viewport: { width: 390, height: 844 },
    isMobile: true,
    hasTouch: true,
    ignoreHTTPSErrors: true,
  });
  page = await context.newPage();
  page.setDefaultTimeout(10000);
  await page.goto(invitation);
  const code = await deadline(
    () => /Approve locally: approve ([a-f0-9]+)/.exec(logs)?.[1],
    "pairing",
  );
  host.stdin.write(`approve ${code}\n`);
  await deadline(
    () =>
      page
        .getByRole("button", { name: "Take control", exact: true })
        .isEnabled(),
    "paired",
  );
  host.stdin.write("apps\n");
  await deadline(() => logs.includes(`"pid": ${app.pid}`), "local discovery");
  const id = createHash("sha256")
    .update(JSON.stringify([app.root, app.pid, app.start]))
    .digest("hex")
    .slice(0, 16);
  host.stdin.write(`share ${id}\n`);
  await page.getByRole("button", { name: /^Viewing:/ }).waitFor();
  await page.getByRole("button", { name: "Take control", exact: true }).click();
  await deadline(
    () =>
      page.getByRole("button", { name: "Release", exact: true }).isEnabled(),
    "control",
  );
  if (files) {
    await page
      .getByRole("button", { name: "Select: example.txt. File", exact: true })
      .click();
    await deadline(
      async () =>
        (await observe(app)).nodes.some(
          (n) => n.label === "Selected: example.txt. File" && !n.actions.length,
        ),
      "native file selection",
    );
    await page
      .getByRole("button", { name: "Selected: example.txt. File", exact: true })
      .waitFor();
    await page
      .getByRole("button", {
        name: "Select: Example folder. Folder",
        exact: true,
      })
      .click();
    await deadline(
      async () =>
        (await observe(app)).nodes.some(
          (n) =>
            n.label === "Selected: Example folder. Folder" && !n.actions.length,
        ),
      "native folder selection",
    );
    // The installed GTK/Nautilus build exposes some menu items without names.
    // Their appearance must not grant a guessed default action to the client.
    await page
      .getByRole("button", { name: "Current Folder Menu", exact: true })
      .click();
    const unnamed = page.getByRole("button", {
      name: "Unnamed control — unavailable",
      exact: true,
    });
    await unnamed.first().waitFor();
    for (const button of await unnamed.all())
      assert.equal(await button.isEnabled(), false);
    await page
      .getByRole("button", { name: "Current Folder Menu", exact: true })
      .click();
    assert.equal(
      await readFile(join(folder, "example.txt"), "utf8"),
      "Temporary file for semantic selection\n",
    );
  } else {
    // The isolated Writer profile has a first-run welcome tour. Handle it through
    // the public UI, demonstrating that its modal correctly blocks the document.
    for (let step = 0; step < 8; step++) {
      const editor = await paragraph(original);
      if (editor && (await editor.isEditable())) break;
      const next = page.getByRole("button", {
        name: /^(Next|Close|Finish|Start Using LibreOffice)$/,
      });
      await deadline(() => next.count(), "welcome action");
      const label = await next.first().innerText();
      await next.first().click();
      await new Promise((r) => setTimeout(r, 600));
      console.log(`Writer welcome: ${label}`);
      if (label !== "Next") break;
    }
    const editor = await deadline(async () => {
      const e = await paragraph(original);
      return e && (await e.isEditable()) ? e : undefined;
    }, "editable Writer paragraph");
    await editor.fill(edited);
    await editor
      .locator("..")
      .getByRole("button", { name: "Apply", exact: true })
      .click();
    // Verify the real application through a separate native read, not the draft.
    await deadline(async () => {
      try {
        return (await observe(app)).nodes.some((n) => n.value === edited);
      } catch {
        return false;
      }
    }, "host document edited");
    const save = page.getByRole("button", { name: "Save", exact: true });
    await deadline(() => save.isEnabled(), "Save enabled");
    await save.click();
    let confirmedFormat = false;
    await deadline(async () => {
      // Writer emits a UTF-8 BOM and a final paragraph newline for plain text.
      if (
        (await readFile(documentPath, "utf8"))
          .replace(/^\uFEFF/, "")
          .replace(/\r?\n$/, "") === edited
      )
        return true;
      const confirm = page.getByRole("button", { name: /^(Use|Keep).*Text/ });
      if (
        !confirmedFormat &&
        (await confirm
          .first()
          .isEnabled({ timeout: 500 })
          .catch(() => false))
      ) {
        await confirm.first().click();
        confirmedFormat = true;
      }
      return false;
    }, "real file saved");
  }
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "existing app content must fit the mobile viewport",
  );
  const out = join(root, "apps/desktop/test-output");
  await mkdir(out, { recursive: true });
  await page.screenshot({
    path: join(out, `${appKind}-${engine}.png`),
    fullPage: true,
  });
  await page.reload();
  const visibleContent = async () =>
    files
      ? await page
          .getByRole("button", {
            name: "Scroll to: example.txt. File",
            exact: true,
          })
          .count()
      : await paragraph(edited);
  await deadline(visibleContent, "reconnect to existing app");
  host.stdin.write(`unshare ${id}\n`);
  await deadline(
    async () => !(await visibleContent()),
    "unshare clears existing app",
  );
  host.stdin.write("quit\n");
  await deadline(() => host.exitCode !== null, "host stopped");
  assert.equal(
    source.exitCode,
    null,
    "host must leave the existing application alive",
  );
  const report = {
    app: files ? "GNOME Files (Nautilus)" : "LibreOffice Writer",
    engine,
    desktop: "existing Wayland session",
    sourceOpenedBeforeHost: true,
    mobileViewport: "390x844",
    pairing: true,
    ...(files
      ? {
          nativeFileSelection: true,
          nativeFolderSelection: true,
          unnamedControlsUnavailable: true,
        }
      : {
          modalOnboarding: true,
          readExistingDocument: true,
          nativeUnicodeEdit: true,
          savedFileVerified: true,
        }),
    reconnect: true,
    unshare: true,
    sourceSurvives: true,
  };
  await writeFile(
    join(out, `${appKind}-${engine}.json`),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
} catch (error) {
  if (lastObservationError)
    console.error("Last observation retry:", lastObservationError);
  console.error(
    "Dispatch diagnostics:",
    logs.split("\n").filter((line) => line.includes("Desktop dispatch:")),
  );
  console.error(
    "Temporary document:",
    JSON.stringify(
      await readFile(documentPath, "utf8").catch(() => "unavailable"),
    ),
  );
  console.error(
    "Client status:",
    await page
      ?.locator("#status")
      .innerText()
      .catch(() => "unavailable"),
  );
  console.error(
    "Client controls:",
    await page
      ?.getByRole("button")
      .allTextContents()
      .catch(() => []),
  );
  throw error;
} finally {
  await browser?.close();
  if (host && host.exitCode === null) {
    host.kill("SIGTERM");
    await deadline(
      () => host.exitCode !== null || host.signalCode !== null,
      "host cleanup",
      10000,
    );
  }
  if (source) {
    try {
      process.kill(-source.pid, "SIGTERM");
    } catch {}
  }
  await proxy?.close();
  await rm(directory, { recursive: true, force: true });
}
