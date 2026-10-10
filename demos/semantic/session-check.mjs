// Owned live host acceptance of the public semantic-only browser API.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import {
  readFile,
  stat,
  mkdir,
  writeFile,
  mkdtemp,
  rm,
  readdir,
} from "node:fs/promises";
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { resolve, sep } from "node:path";
import { chromium } from "../../client-browser/node_modules/playwright/index.mjs";
const root = resolve(import.meta.dirname, "../..");
assert.equal(process.env.RUI_ISOLATED_DISPLAY, "1", "owned display required");
const output = process.env.RUI_SEMANTIC_OUTPUT;
assert(output, "new output directory required");
await mkdir(output, { recursive: false });
const temp = await mkdtemp(resolve(tmpdir(), "rui-semantic-session-"));
const app = process.env.RUI_SEMANTIC_APPLICATION ?? "corpus";
assert(["corpus", "editor"].includes(app));
const publicRoot = resolve(root, "client-browser");
const server = createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.writeHead(200, { "Content-Type": "text/html" });
      res.end("<!doctype html><title>Owned semantic session</title>");
      return;
    }
    const path = resolve(
      publicRoot,
      "." + new URL(req.url, "http://localhost").pathname,
    );
    if (!path.startsWith(publicRoot + sep) || !(await stat(path)).isFile())
      throw Error();
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
const wait = async (read, condition = Boolean) => {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      const value = await read();
      if (condition(value)) return value;
    } catch {}
    await new Promise((r) => setTimeout(r, 25));
  }
  throw Error("convergence deadline");
};
let browser,
  host,
  logs = "";
try {
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const origin = `http://127.0.0.1:${server.address().port}`,
    identity = resolve(temp, "identity");
  host = spawn(
    resolve(root, "target/release/rui-demo"),
    [
      "--host-only",
      "--application",
      app,
      "--identity-dir",
      identity,
      "--browser-origin",
      origin,
    ],
    { cwd: root, env: process.env, stdio: ["pipe", "pipe", "pipe"] },
  );
  for (const stream of [host.stdout, host.stderr])
    stream.on("data", (b) => (logs = (logs + b).slice(-16384)));
  const port = await wait(
    () => /Visible GTK host listening at 127.0.0.1:(\d+)/.exec(logs)?.[1],
  );
  const key = execFileSync("openssl", [
    "x509",
    "-in",
    resolve(identity, "host.pem"),
    "-pubkey",
    "-noout",
  ]);
  const der = execFileSync("openssl", ["pkey", "-pubin", "-outform", "DER"], {
    input: key,
  });
  const pin = createHash("sha256").update(der).digest("base64");
  browser = await chromium.launch({
    headless: true,
    executablePath: process.env.RUI_WASM_CHROMIUM,
    args: ["--ignore-certificate-errors-spki-list=" + pin],
  });
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(origin);
  const credential = await readFile(resolve(identity, "client.credential"));
  const result = await page.evaluate(
    async ({ url, credential }) => {
      const { BrowserClient, webSocketTransport, text, SemanticView } =
        await import("/dist/index.js");
      const root = document.createElement("div");
      document.body.append(root);
      const view = new SemanticView(root);
      const evidence = [],
        events = [];
      let wireBytes = 0;
      function field(bytes, tag) {
        let at = 0;
        const uint = () => {
          let out = 0n,
            shift = 0n;
          while (at < bytes.length) {
            const b = bytes[at++];
            out |= BigInt(b & 127) << shift;
            if (!(b & 128)) return out;
            shift += 7n;
            if (shift > 63n) throw Error("varint");
          }
          throw Error("truncated");
        };
        while (at < bytes.length) {
          const key = Number(uint()),
            type = key & 7,
            n = key >> 3;
          let value;
          if (type === 0) value = uint();
          else if (type === 2) {
            const length = Number(uint());
            value = bytes.slice(at, at + length);
            at += length;
          } else if (type === 1) at += 8;
          else throw Error("wire type");
          if (n === tag) return value;
        }
        return undefined;
      }
      function record(direction, bytes) {
        const kind = Number(field(bytes, 5));
        if (kind === 1) {
          evidence.push({ direction, kind, redacted: true });
          return;
        }
        wireBytes += bytes.length;
        if (evidence.length >= 256 || wireBytes > 2 << 20)
          throw Error("evidence ceiling");
        evidence.push({
          direction,
          kind,
          hex: [...bytes].map((n) => n.toString(16).padStart(2, "0")).join(""),
        });
      }
      const transport = (url, cbs) => {
        const inner = webSocketTransport(url, {
          ...cbs,
          message(bytes) {
            record("received", new Uint8Array(bytes));
            cbs.message(bytes);
          },
        });
        return {
          get bufferedAmount() {
            return inner.bufferedAmount;
          },
          send(bytes) {
            record("sent", bytes);
            inner.send(bytes);
          },
          close() {
            inner.close();
          },
        };
      };
      const client = await BrowserClient.create({
        mode: "semanticOnly",
        url,
        clientId: new Uint8Array(16).fill(3),
        credential: async () => new Uint8Array(credential),
        selection: "fixture",
        transport,
        onEvent(event) {
          events.push({ ...event });
          if (event.type === "applied" || event.type === "resumed") {
            const state = client.state;
            if (state) {
              view.render(state);
              client.semanticReady(state.revision);
            }
          } else if (event.type === "reset" || event.type === "disconnected") {
            view.reset();
          }
        },
      });
      const wait = async (condition) => {
        const deadline = performance.now() + 12000;
        while (performance.now() < deadline) {
          if (condition()) return;
          await new Promise((r) => setTimeout(r, 10));
        }
        throw Error(
          "semantic convergence " +
            JSON.stringify(events, (_, v) =>
              typeof v === "bigint" ? String(v) : v,
            ),
        );
      };
      const nodes = () => {
        const state = client.state,
          found = [],
          seen = new Set();
        function visit(ref) {
          if (!ref || seen.has(ref.id)) return;
          seen.add(ref.id);
          const record = state.records.get(ref.id);
          if (record && "Semantic" in record) {
            const n = record.Semantic;
            found.push(n);
            for (const child of n.children) visit(child);
          }
        }
        for (const record of state?.records.values() ?? [])
          if (
            "Surface" in record &&
            record.Surface.mapped &&
            !record.Surface.parent
          )
            visit(record.Surface.semantic_root);
        return found;
      };
      const enabled = (n) =>
        n.states.get("Disabled") === undefined ||
        n.states.get("Disabled") === "False";
      const action = (n, id) =>
        n.actions.some((a) => a.id === id && a.schema_version === 1);
      const settled = () => client.pendingCommands === 0 && client.hasControl;
      await client.connect();
      await wait(() => client.state && evidence.some((p) => p.kind === 35));
      let button = nodes().find(
        (n) => n.role === "Button" && enabled(n) && action(n, "Activate"),
      );
      if (!button) throw Error("no semantic button");
      let observerRejected = false;
      try {
        client.activate(button.id);
      } catch {
        observerRejected = true;
      }
      if (!observerRejected) throw Error("observer dispatch");
      client.requestControl();
      await wait(() => client.hasControl);
      const effects = [];
      const seq = client.activate(button.id);
      effects.push({ operation: "Activate", target: button.id, sequence: seq });
      await wait(settled);
      const entry = nodes().find(
        (n) =>
          n.role === "TextField" &&
          enabled(n) &&
          action(n, "Focus") &&
          action(n, "ReplaceText") &&
          text(client.state, n.id)?.editable,
      );
      if (!entry) throw Error("no editable semantic field");
      client.focus(entry.id);
      await wait(settled);
      const capture = client.captureEdit(entry.id),
        content = "Semantic A😀éאב";
      effects.push({
        operation: "ReplaceText",
        target: entry.id,
        sequence: client.commitEdit(capture, content),
      });
      await wait(settled);
      if (text(client.state, entry.id)?.content !== content)
        throw Error("Unicode text diverged");
      const collection = nodes().find((n) => n.collection?.items.length > 1);
      if (!collection) throw Error("no materialized collection");
      const item = collection.collection.items.find(
        (ref) =>
          !collection.collection.selected_items.some(
            (selected) => selected.id === ref.id,
          ),
      );
      let staleModelRejected = false;
      try {
        client.activateItem(
          collection.id,
          item.id,
          collection.collection.version - 1n,
        );
      } catch {
        staleModelRejected = true;
      }
      if (!staleModelRejected) throw Error("stale collection model accepted");
      effects.push({
        operation: "ActivateItem",
        target: item.id,
        sequence: client.activateItem(
          collection.id,
          item.id,
          collection.collection.version,
        ),
      });
      await wait(settled);
      let invalidValueRejected = false;
      try {
        client.setChecked(button.id, true);
      } catch {
        invalidValueRejected = true;
      }
      if (!invalidValueRejected) throw Error("value schema mismatch accepted");
      const checkbox = nodes().find(
        (n) =>
          n.role === "Checkbox" &&
          action(n, "SetValue") &&
          n.value &&
          "Boolean" in n.value,
      );
      let valueEffect = null;
      if (checkbox) {
        valueEffect = {
          target: checkbox.id,
          sequence: client.setChecked(checkbox.id, true),
        };
        await wait(settled);
        if (!nodes().find((n) => n.id === checkbox.id)?.value?.Boolean)
          throw Error(
            "native boolean not applied " +
              JSON.stringify(events.slice(-8), (_, v) =>
                typeof v === "bigint" ? String(v) : v,
              ),
          );
      }
      let state = client.state;
      for (const record of state.records.values()) {
        if ("Scene" in record || "Binding" in record)
          throw Error("visual records in semantic projection");
        if ("Session" in record && record.Session.cursor)
          throw Error("cursor in semantic projection");
        if ("Surface" in record && record.Surface.scene_root)
          throw Error("visual surface routes in semantic projection");
      }
      if (
        evidence.some((p) =>
          [20, 21, 22, 23, 24, 31, 51, 52, 53].includes(p.kind),
        )
      )
        throw Error("visual/resource/other family traffic");
      const readOnlyDom = {
        nodes: root.querySelectorAll("[data-semantic-id]").length,
        fields: root.querySelectorAll("textarea").length,
        allFieldsReadOnly: [...root.querySelectorAll("textarea")].every(
          (e) => e.readOnly,
        ),
      };
      if (
        readOnlyDom.nodes < 5 ||
        !readOnlyDom.fields ||
        !readOnlyDom.allFieldsReadOnly
      )
        throw Error("live read-only semantic DOM");
      const capabilities = client.capabilities;
      let revokedValueRejected = null;
      if (checkbox) {
        client.releaseControl();
        try {
          client.setChecked(checkbox.id, false);
          revokedValueRejected = false;
        } catch {
          revokedValueRejected = true;
        }
        if (!revokedValueRejected) throw Error("revoked value dispatch");
      }
      client.disconnect();
      const reconnectDeadline = performance.now() + 5000;
      while (true) {
        try {
          await client.connect();
          break;
        } catch (error) {
          if (
            !String(error).includes("still closing") ||
            performance.now() > reconnectDeadline
          )
            throw error;
          await new Promise((r) => setTimeout(r, 50));
        }
      }
      await wait(() => client.state);
      if (client.hasControl) throw Error("authority restored on reconnect");
      state = client.state;
      client.dispose();
      view.dispose();
      if (root.childNodes.length)
        throw Error("authorized DOM retained after disposal");
      const normalize = (v) =>
        typeof v === "bigint"
          ? v.toString()
          : v instanceof Map
            ? Object.fromEntries(
                [...v].map(([k, v]) => [String(k), normalize(v)]),
              )
            : Array.isArray(v)
              ? v.map(normalize)
              : v && typeof v === "object"
                ? Object.fromEntries(
                    Object.entries(v).map(([k, v]) => [k, normalize(v)]),
                  )
                : v;
      return normalize({
        passed: true,
        observerRejected,
        canvasConstructed: false,
        readOnlyDom,
        capabilities,
        effects,
        valueEffect,
        staleModelRejected,
        invalidValueRejected,
        revokedValueRejected,
        state,
        evidence,
        events,
      });
    },
    { url: `wss://127.0.0.1:${port}/rui`, credential: [...credential] },
  );
  credential.fill(0);
  assert.equal(errors.length, 0, errors.join("\n"));
  const source = resolve(identity, "source");
  const files = (await readdir(source))
    .filter((n) => /^whole-\d+\.json$/.test(n))
    .sort();
  const final = JSON.parse(
    await readFile(resolve(source, files.at(-1)), "utf8"),
  );
  assert(
    final.semantics.some(
      (n) =>
        n.label ===
        (app === "corpus" ? "Host state: 1 clicks" : "Saved 1 times"),
    ),
    "one native button effect",
  );
  assert(
    final.semantics.some((n) => n.collection_model?.selected_items.length),
    "source stable selection missing",
  );
  await writeFile(
    resolve(output, "session.json"),
    JSON.stringify(
      {
        ...result,
        application: app,
        browser: browser.version(),
        pinScope: "owned-test-certificate-SPKI",
        sourceRevision: final.source_revision,
      },
      null,
      2,
    ) + "\n",
  );
  console.log(
    JSON.stringify({
      passed: true,
      application: app,
      browser: browser.version(),
      effects: result.effects.length,
      canvas: false,
      presented: false,
      resources: false,
      reconnect: true,
    }),
  );
} catch (error) {
  console.error(logs.slice(-4000));
  throw error;
} finally {
  await browser?.close();
  host?.stdin.write("\n");
  if (host)
    await new Promise((r) => {
      host.once("exit", r);
      setTimeout(() => {
        host.kill("SIGKILL");
        r();
      }, 3000).unref();
    });
  await new Promise((r) => server.close(r));
  await rm(temp, { recursive: true, force: true });
}
