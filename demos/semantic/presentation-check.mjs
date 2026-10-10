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
const temp = await mkdtemp(resolve(tmpdir(), "rui-semantic-team-"));
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
      "--second-client",
      ...(app === "editor" ? ["--partial-collection"] : []),
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
  const second = await readFile(resolve(identity, "second-client.credential"));
  await page.evaluate(
    async ({ url, credentials, application }) => {
      const {
        BrowserClient,
        webSocketTransport,
        SemanticControls,
        semantic,
        text,
      } = await import("/dist/index.js");
      const team = [];
      const varint = (b, at) => {
        let n = 0n,
          s = 0n;
        while (at < b.length) {
          const c = b[at++];
          n |= BigInt(c & 127) << s;
          if (!(c & 128)) return [n, at];
          s += 7n;
          if (s > 63n) throw Error("varint");
        }
        throw Error("truncated");
      };
      const kind = (b) => {
        let at = 0;
        while (at < b.length) {
          let key;
          [key, at] = varint(b, at);
          const type = Number(key & 7n);
          if (type === 0) {
            let n;
            [n, at] = varint(b, at);
            if (key >> 3n === 5n) return Number(n);
          } else if (type === 2) {
            let n;
            [n, at] = varint(b, at);
            at += Number(n);
          } else if (type === 1) at += 8;
          else throw Error("wire");
        }
        throw Error("kind");
      };
      for (let i = 0; i < 2; i++) {
        const evidence = [],
          events = [],
          effects = [];
        let bytes = 0;
        const record = (direction, data) => {
          const b = new Uint8Array(data),
            k = kind(b);
          if (k === 1) {
            evidence.push({ direction, kind: k, redacted: true });
            return;
          }
          bytes += b.length;
          if (bytes > 4 << 20 || evidence.length >= 1024)
            throw Error("evidence limit");
          evidence.push({
            direction,
            kind: k,
            hex: [...b].map((n) => n.toString(16).padStart(2, "0")).join(""),
          });
        };
        const traffic = {
          hold: false,
          frames: [],
          bytes: 0,
          deliver: undefined,
        };
        const transport = (url, callbacks) => {
          traffic.deliver = callbacks.message;
          const ws = webSocketTransport(url, {
            ...callbacks,
            message(b) {
              record("received", b);
              if (traffic.hold) {
                traffic.bytes += b.byteLength;
                if (traffic.frames.length >= 32 || traffic.bytes > 1 << 20)
                  throw Error("delayed delivery limit");
                traffic.frames.push(b.slice(0));
                return;
              }
              callbacks.message(b);
            },
          });
          return {
            get bufferedAmount() {
              return ws.bufferedAmount;
            },
            send(b) {
              record("sent", b);
              ws.send(b);
            },
            close() {
              ws.close();
            },
          };
        };
        const root = document.createElement("div"),
          status = document.createElement("p");
        document.body.append(status, root);
        const canvas = i ? document.createElement("canvas") : undefined;
        if (canvas) root.append(canvas);
        const client = await BrowserClient.create({
          url,
          clientId: new Uint8Array(16).fill(i + 3),
          credential: async () => new Uint8Array(credentials[i]),
          selection: "fixture",
          transport,
          ...(i ? { mode: "mirror", canvas } : { mode: "semanticOnly" }),
          onEvent: (e) => events.push({ ...e }),
        });
        const controls = i
          ? undefined
          : new SemanticControls(client, root, status);
        team.push({
          client,
          root,
          status,
          canvas,
          controls,
          evidence,
          events,
          effects,
          traffic,
        });
      }
      const wait = async (fn) => {
        const end = Date.now() + 15000;
        while (Date.now() < end) {
          if (fn()) return;
          await new Promise((r) => setTimeout(r, 20));
        }
        throw Error(
          "client convergence " +
            team
              .map((t) =>
                JSON.stringify(t.events.slice(-4), (_k, v) =>
                  typeof v === "bigint" ? String(v) : v,
                ),
              )
              .join("\n"),
        );
      };
      const nodes = (t) => {
        const state = t.client.state,
          out = [],
          seen = new Set();
        if (!state) return out;
        function visit(ref) {
          if (!ref || seen.has(ref.id)) return;
          seen.add(ref.id);
          const n = semantic(state, ref.id);
          if (!n) return;
          out.push(n);
          n.children.forEach(visit);
        }
        for (const r of state.records.values())
          if ("Surface" in r && r.Surface.mapped && !r.Surface.parent)
            visit(r.Surface.semantic_root);
        return out;
      };
      const action = (n, a) =>
        n.actions.some((x) => x.id === a && x.schema_version === 1);
      const button = (t) =>
        nodes(t).find((n) => n.role === "Button" && action(n, "Activate"));
      const field = (t) =>
        nodes(t).find(
          (n) =>
            n.role === "TextField" &&
            action(n, "Focus") &&
            action(n, "ReplaceText") &&
            text(t.client.state, n.id)?.editable,
        );
      const values = (t) =>
        nodes(t).map((n) => ({
          role: n.role,
          label: n.label,
          text: text(t.client.state, n.id)?.content ?? null,
          value: n.value,
          items: n.collection?.items.length ?? null,
          selected:
            n.collection?.selected_items.map((ref) =>
              n.collection.items.findIndex((x) => x.id === ref.id),
            ) ?? null,
        }));
      const normalize = (v) =>
        typeof v === "bigint"
          ? String(v)
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
      const same = () =>
        JSON.stringify(normalize(values(team[0]))) ===
        JSON.stringify(normalize(values(team[1])));
      const settled = (t) =>
        t.client.hasControl && t.client.pendingCommands === 0;
      window.team = {
        application,
        credentials,
        team,
        wait,
        nodes,
        action,
        button,
        field,
        values,
        normalize,
        same,
        settled,
        text,
      };
      for (const t of team) await t.client.connect();
      await wait(
        () =>
          team[0].evidence.some((p) => p.kind === 35) &&
          team[1].events.some((e) => e.type === "presented") &&
          same(),
      );
    },
    {
      url: `wss://127.0.0.1:${port}/rui`,
      credentials: [[...credential], [...second]],
      application: app,
    },
  );
  credential.fill(0);
  second.fill(0);
  if (process.env.RUI_RESOURCE_RECOVERY === "1") {
    await page.evaluate(async () => {
      const { team, wait, normalize, same } = window.team,
        [, m] = team;
      m.client.requestControl();
      await wait(() => m.client.hasControl);
      const state = m.client.state,
        before = JSON.stringify(normalize(state));
      const presented = m.events.filter((e) => e.type === "presented").length;
      const glyph = [...state.records.values()].find(
        (r) =>
          "Scene" in r &&
          typeof r.Scene.primitive === "object" &&
          r.Scene.primitive.GlyphRun,
      )?.Scene.primitive.GlyphRun;
      if (!glyph || !("Outline" in glyph.font))
        throw Error("source outline body missing");
      const digest = new Uint8Array(glyph.font.Outline),
        owned = m.client.resourceBytes(digest),
        first = owned[0];
      owned[0] ^= 255;
      if (m.client.resourceBytes(digest)[0] !== first)
        throw Error("resource getter returned mutable live bytes");
      m.client.resourceCacheLost();
      if (
        m.client.hasControl ||
        m.client.resourcesReady ||
        JSON.stringify(normalize(m.client.state)) !== before
      )
        throw Error("cache loss did not preserve records and pause authority");
      m.client.refresh();
      await wait(
        () =>
          m.client.resourcesReady &&
          m.client.hasControl &&
          m.events.filter((e) => e.type === "presented").length > presented &&
          same(),
      );
      if (JSON.stringify(normalize(m.client.state)) !== before)
        throw Error("cache recovery changed committed records");
      if (m.client.resourceBytes(digest)[0] !== first)
        throw Error("cache refill did not restore exact verified bytes");
      m.client.releaseControl();
      await wait(() => !m.client.hasControl);
      window.team.resourceRecovery = true;
    });
  }

  await page.evaluate(async () => {
    const { team, wait, button, field, nodes, action, settled, same, text } =
        window.team,
      [s, m] = team;
    s.client.requestControl();
    await wait(() => s.client.hasControl);
    if (m.client.hasControl) throw Error("two controllers");
    const b = button(s);
    s.traffic.hold = true;
    s.root
      .querySelector(`[data-semantic-id="${b.id}"] [data-action=Activate]`)
      .click();
    s.effects.push({ operation: "Activate" });
    await wait(
      () =>
        s.traffic.frames.length > 0 &&
        nodes(m).some(
          (n) =>
            n.label ===
            (window.team.application === "editor"
              ? "Saved 1 times"
              : "Host state: 1 clicks"),
        ),
    );
    if (!s.client.pendingCommands) throw Error("delayed result lost command");
    let pendingRejected = false;
    try {
      s.client.activate(b.id);
    } catch {
      pendingRejected = true;
    }
    if (!pendingRejected)
      throw Error("delayed command admitted a fresh operation");
    s.traffic.hold = false;
    for (const frame of s.traffic.frames.splice(0)) s.traffic.deliver(frame);
    s.traffic.bytes = 0;
    await wait(() => settled(s) && same());
    const f = field(s);
    s.root
      .querySelector(`[data-semantic-id="${f.id}"] [data-action=EditDraft]`)
      .click();
    await wait(() => settled(s) && s.root.querySelector("[data-local-draft]"));
    const draft = s.root.querySelector("[data-local-draft]");
    draft.value = "Semantic team 𝄞 é אב";
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    s.root.querySelector("[data-action=ApplyDraft]").click();
    s.effects.push({ operation: "ReplaceText" });
    await wait(
      () =>
        settled(s) &&
        same() &&
        text(s.client.state, f.id).content === draft.value,
    );
    const list = nodes(s).find((n) => n.collection?.items.length > 1);
    const item = list.collection.items.at(-1);
    s.root
      .querySelector(`[data-semantic-id="${item.id}"] [data-action=Activate]`)
      .click();
    s.effects.push({ operation: "ActivateItem" });
    await wait(() => settled(s) && same());
    const next = field(s);
    s.root
      .querySelector(`[data-semantic-id="${next.id}"] [data-action=EditDraft]`)
      .click();
    await wait(() => settled(s) && s.root.querySelector("[data-local-draft]"));
    s.root.querySelector("[data-local-draft]").value =
      "never submit after revocation";
    s.client.releaseControl();
    await wait(
      () => !s.client.hasControl && !s.root.querySelector("[data-local-draft]"),
    );
    m.client.requestControl();
    await wait(() => m.client.hasControl);
    if (s.client.hasControl) throw Error("authority survived handoff");
    let rejected = false;
    try {
      s.client.activate(button(s).id);
    } catch {
      rejected = true;
    }
    if (!rejected) throw Error("semantic observer mutated");
    m.effects.push({
      operation: "Activate",
      sequence: m.client.activate(button(m).id),
    });
    await wait(() => settled(m) && same());
    const mf = field(m);
    m.client.focus(mf.id);
    await wait(() => settled(m));
    const capture = m.client.captureEdit(mf.id);
    m.effects.push({
      operation: "ReplaceText",
      sequence: m.client.commitEdit(capture, "Mirror team 𝄞 שלום"),
    });
    await wait(() => settled(m) && same());
    const ml = nodes(m).find((n) => n.collection?.items.length > 1);
    m.effects.push({
      operation: "ActivateItem",
      sequence: m.client.activateItem(
        ml.id,
        ml.collection.items[0].id,
        ml.collection.version,
      ),
    });
    await wait(() => settled(m) && same());
    m.client.releaseControl();
    await wait(() => !m.client.hasControl);
    s.controls.consumeCurrent();
    s.client.requestControl();
    await wait(() => s.client.hasControl);
    if (window.team.application === "editor") {
      const collection = nodes(s).find((n) => n.collection);
      if (
        collection.collection.total_count !== 10n ||
        collection.collection.items.length !== 2 ||
        collection.collection.materialization !== "Partial"
      )
        throw Error("partial source collection omitted");
      const buttons = nodes(s).filter(
        (n) => n.role === "Button" && action(n, "Activate"),
      );
      s.client.activate(buttons[1].id);
      await wait(
        () =>
          settled(s) &&
          same() &&
          [...s.client.state.records.values()].some(
            (r) => "Surface" in r && r.Surface.mapped && r.Surface.modal_for,
          ),
      );
      if (!s.root.querySelector("[role=dialog]"))
        throw Error("native modal omitted from semantic DOM");
      const modal = [...s.client.state.records.values()].find(
        (r) => "Surface" in r && r.Surface.mapped && r.Surface.modal_for,
      ).Surface;
      const state = s.client.state;
      const candidates = [];
      const visit = (ref) => {
        const n = state.records.get(ref.id)?.Semantic;
        if (n) {
          candidates.push(n);
          n.children.forEach(visit);
        }
      };
      visit(modal.semantic_root);
      const close = candidates.find(
        (n) => n.role === "Button" && action(n, "Activate"),
      );
      s.client.activate(close.id);
      await wait(
        () => settled(s) && same() && !s.root.querySelector("[role=dialog]"),
      );
      const old = field(s).id;
      s.client.activate(buttons[2].id);
      await wait(
        () => settled(s) && same() && !s.client.state.records.has(old),
      );
      if (
        s.root.querySelector(`[data-semantic-id="${old}"]`) ||
        !s.client.state.retired.includes(old)
      )
        throw Error("removed control retained");
      if (
        !nodes(s).some(
          (n) =>
            n.role === "TextField" && action(n, "ReplaceText") && n.id !== old,
        )
      )
        throw Error("replacement field absent");
      window.team.modalAndReplacement = true;
    }
    const sf = field(s);
    s.client.focus(sf.id);
    await wait(() => settled(s));
    window.team.capture = s.client.captureEdit(sf.id);
  });
  let sourceDirectory = resolve(identity, "source");
  const newest = async () => {
    const files = (await readdir(sourceDirectory))
      .filter((n) => /^whole-\d+\.json$/.test(n))
      .sort();
    return JSON.parse(
      await readFile(resolve(sourceDirectory, files.at(-1)), "utf8"),
    );
  };
  const before = await newest();
  for (const width of [320, 800, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(() => {
      const m = window.team.team[1];
      m.client.setViewport({ ...m.client.geometry, width: 480, height: 360 });
    });
    const current = await newest();
    assert.equal(
      current.source_revision,
      before.source_revision,
      "local viewport mutated source",
    );
    assert.deepEqual(
      current.surfaces.map((s) => ({ size: s.size, origin: s.origin })),
      before.surfaces.map((s) => ({ size: s.size, origin: s.origin })),
      "local viewport changed source layout",
    );
  }

  execFileSync(
    "python3",
    [
      resolve(root, "fixtures/render/native_events.py"),
      app === "editor" ? "RUI document editor" : "RUI Host",
      "resize_by",
      "32",
      "20",
    ],
    {
      env: process.env,
      timeout: 5000,
    },
  );
  await wait(
    newest,
    (f) =>
      f.source_revision > before.source_revision &&
      JSON.stringify(f.surfaces.map((s) => s.size)) !==
        JSON.stringify(before.surfaces.map((s) => s.size)),
  );
  await page.evaluate(async () => {
    const { team, wait, same, settled, capture } = window.team,
      [s] = team;
    await wait(() => settled(s) && same());
    let rejected = false;
    try {
      s.client.commitEdit(capture, "stale geometry capture");
    } catch {
      rejected = true;
    }
    if (!rejected) throw Error("stale capture sent");
    window.team.staleRejected = true;
    s.client.releaseControl();
    await wait(() => !s.client.hasControl);
    s.client.disconnect();
    await wait(() => s.root.childNodes.length === 0);
    for (let i = 0; i < 10; i++) {
      try {
        await s.client.connect();
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    await wait(
      () => s.client.state && s.evidence.some((p) => p.kind === 35) && same(),
    );
    if (s.client.hasControl) throw Error("reconnect restored control");
  });
  const final = await newest();
  assert(
    final.semantics.some(
      (n) =>
        n.label ===
        (app === "editor" ? "Saved 2 times" : "Host state: 2 clicks"),
    ),
    "exact two native button effects",
  );
  await page.evaluate(async () => {
    const { team, wait, field, settled } = window.team,
      [s] = team;
    s.controls.consumeCurrent();
    s.client.requestControl();
    await wait(() => s.client.hasControl);
    const f = field(s);
    s.root
      .querySelector(`[data-semantic-id="${f.id}"] [data-action=EditDraft]`)
      .click();
    await wait(() => settled(s) && s.root.querySelector("[data-local-draft]"));
    window.team.oldSourceTarget = f.id;
    window.team.oldSourceRevision = s.client.state.revision;
    s.root.querySelector("[data-local-draft]").value =
      "source incarnation must retire this draft";
  });
  host.stdin.write("restart\n");
  await wait(() => logs.includes("Owned source replaced"));
  sourceDirectory = resolve(identity, "source-replacement-1");
  await page.evaluate(async () => {
    const { team, wait, same, oldSourceTarget, oldSourceRevision } =
        window.team,
      [s, m] = team;
    await wait(
      () =>
        s.client.state?.revision > oldSourceRevision &&
        same() &&
        s.root.querySelector("[data-semantic-id]"),
    );
    if (
      s.client.hasControl ||
      m.client.hasControl ||
      s.root.querySelector("[data-local-draft]") ||
      s.client.state.records.has(oldSourceTarget)
    )
      throw Error("source replacement retained old authority/target/draft");
    if (!s.client.state.retired.includes(oldSourceTarget))
      throw Error("source target did not retire");
    window.team.sourceRestart = true;
  });
  await page.evaluate(async () => {
    const { team, wait, field, settled } = window.team,
      [s] = team;
    s.controls.consumeCurrent();
    s.client.requestControl();
    await wait(() => s.client.hasControl);
    const f = field(s);
    s.root
      .querySelector(`[data-semantic-id="${f.id}"] [data-action=EditDraft]`)
      .click();
    await wait(() => settled(s) && s.root.querySelector("[data-local-draft]"));
  });
  host.stdin.write("observer-first\n");
  await wait(() => logs.includes("First client permission reduced"));
  await page.evaluate(async () => {
    const { team, wait } = window.team,
      [s] = team;
    await wait(
      () =>
        s.client.connectionState === "disconnected" &&
        !s.root.querySelector("[data-local-draft]"),
    );
  });
  const observerCredential = await readFile(
    resolve(identity, "client.credential"),
  );
  await page.evaluate(
    async (raw) => {
      const { team, credentials, wait, same } = window.team,
        [s] = team;
      credentials[0].fill(0);
      credentials[0] = raw;
      for (let i = 0; i < 10; i++) {
        try {
          await s.client.connect();
          break;
        } catch {
          await new Promise((r) => setTimeout(r, 100));
        }
      }
      await wait(() => s.client.state && same());
      if (
        s.client.hasControl ||
        s.client.capabilities.can_control ||
        s.client.capabilities.actions.length
      )
        throw Error("permission reduction did not persist");
      window.team.permissionReduction = true;
    },
    [...observerCredential],
  );
  observerCredential.fill(0);
  await page.evaluate(async () => {
    const { team, wait, button, settled, same } = window.team,
      [, m] = team;
    m.client.requestControl();
    await wait(() => m.client.hasControl);
    m.client.activate(button(m).id);
    await wait(() => settled(m) && same());
    m.client.releaseControl();
    await wait(() => !m.client.hasControl);
  });
  const replacementFinal = await newest();
  assert(
    replacementFinal.semantics.some(
      (n) =>
        n.label ===
        (app === "editor" ? "Saved 1 times" : "Host state: 1 clicks"),
    ),
    "one replacement-source effect after authority reduction",
  );
  const reports = await page.evaluate(() => {
    const { team, normalize, same, staleRejected } = window.team;
    if (!same()) throw Error("authorized source values differ");
    return team.map((t, i) =>
      normalize({
        passed: true,
        mode: i ? "mirror" : "semanticOnly",
        clientId: (i ? "04" : "03").repeat(16),
        state: t.client.state,
        evidence: t.evidence,
        events: t.events,
        effects: t.effects,
        staleRejected,
        sourceRestart: window.team.sourceRestart,
        permissionReduction: window.team.permissionReduction,
        revocationPurged: true,
        controlSwapped: true,
        authorizedProjectionConverged: true,
        delayedResultsIsolated: true,
        resourceRecovery: window.team.resourceRecovery ?? false,
        localViewports: [320, 800, 1440],
        modalAndReplacement: window.team.modalAndReplacement ?? false,
        partialCollection: window.team.application === "editor",
      }),
    );
  });
  for (const r of reports) {
    await writeFile(
      resolve(output, r.mode + ".json"),
      JSON.stringify(
        {
          ...r,
          application: app,
          browser: browser.version(),
          sourceRevision: replacementFinal.source_revision,
          initialSourceRevision: final.source_revision,
          initialNativeCounterEffects: 2,
          replacementNativeCounterEffects: 1,
        },
        null,
        2,
      ) + "\n",
    );
  }
  const forbidden = [...reports[0].evidence].filter(
    (p) => p.direction === "sent" && [31, 51, 52, 53, 54].includes(p.kind),
  );
  assert.equal(forbidden.length, 0, "semantic visual/raw traffic");
  assert(
    !Object.values(reports[0].state.records).some((r) => r.Scene || r.Binding),
  );
  assert(reports[1].events.some((e) => e.type === "presented"));
  await page.evaluate(() => {
    for (const t of window.team.team) {
      t.controls?.dispose();
      t.client.dispose();
    }
  });
  assert.equal(errors.length, 0, errors.join("\n"));
  console.log(
    JSON.stringify({
      passed: true,
      application: app,
      clients: 2,
      distinctCredentials: true,
      controlSwapped: true,
      converged: true,
      sourceGeometryChanged: true,
    }),
  );
} catch (error) {
  const p = browser?.contexts()[0]?.pages()[0];
  if (p)
    console.error(
      await p.evaluate(() =>
        window.team?.team.map((t) => ({
          events: t.events.slice(-8),
          surfaces: [...(t.client.state?.records.values() ?? [])].filter(
            (r) => "Surface" in r,
          ),
        })),
      ),
    );
  try {
    const files = (await readdir(resolve(temp, "identity/source")))
      .filter((n) => /^whole-\d+\.json$/.test(n))
      .sort();
    const frame = JSON.parse(
      await readFile(resolve(temp, "identity/source", files.at(-1)), "utf8"),
    );
    console.error(
      JSON.stringify({
        coverage: frame.coverage,
        gaps: frame.gaps,
        surfaces: frame.surfaces.map((s) => ({
          key: s.surface_key,
          completeness: s.completeness,
          gaps: s.gaps,
        })),
      }),
    );
  } catch {}
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
