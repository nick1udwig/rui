import init, { Receiver, Session } from "../wasm/rui_client_wasm.js";
import { BrowserClient } from "../dist/client.js";
import { SemanticView } from "../dist/semantic.js";
const assert = (ok, message) => {
  if (!ok) throw Error(message);
};
const enc = new TextEncoder();
const join = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};
const uint = (value) => {
  value = BigInt(value);
  const out = [];
  do {
    out.push(Number(value & 127n) | (value >= 128n ? 128 : 0));
    value >>= 7n;
  } while (value);
  return new Uint8Array(out);
};
const scalar = (tag, v) => join(uint(tag * 8), uint(v));
const blob = (tag, v) => {
  v = typeof v === "string" ? enc.encode(v) : v;
  return join(uint(tag * 8 + 2), uint(v.length), v);
};
const ids = [1, 2, 3, 4].map((n) => new Uint8Array(16).fill(n));
const packet = (kind, body) =>
  join(
    blob(1, ids[0]),
    blob(2, ids[1]),
    blob(3, ids[3]),
    scalar(4, 1),
    scalar(5, kind),
    blob(6, body),
  );
function canonical(value) {
  if (typeof value === "number" || typeof value === "bigint")
    return String(value);
  if (value instanceof Map)
    return canonical(
      Object.fromEntries([...value].map(([k, v]) => [String(k), v])),
    );
  if (Array.isArray(value) || value instanceof Set)
    return [...value].map(canonical);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((k) => [k, canonical(value[k])]),
    );
  return value;
}
export async function checkContract() {
  await init();
  const fixture = await (await fetch("/wasm/contract.json")).json();
  const payload = new Uint8Array(
    fixture.hex.match(/../g).map((x) => parseInt(x, 16)),
  );
  const receiver = new Receiver(...ids, 32 << 20, false);
  try {
    const envelope = join(
      scalar(1, 1),
      blob(2, ids[0]),
      blob(3, ids[1]),
      blob(4, ids[3]),
      scalar(5, 1),
      blob(6, payload),
    );
    receiver.receiveEnvelope(ids[0], envelope);
    const state = receiver.state();
    assert(
      JSON.stringify(canonical(state)) ===
        JSON.stringify(canonical(fixture.state)),
      "complete native/Python/WASM state differs",
    );
    assert(
      new Set([...state.records.values()].map((r) => Object.keys(r)[0]))
        .size === 6,
      "record variant omitted",
    );
    assert(state.revision === 0xffffffffffffffffn, "maximum clock rounded");
    const values = [...state.records.values()]
      .filter((r) => "Semantic" in r)
      .map((r) => r.Semantic.value);
    assert(
      values.some((v) => v?.Integer === -0x8000000000000000n),
      "signed integer rounded",
    );
    const root = document.createElement("div");
    document.body.append(root);
    const view = new SemanticView(root);
    view.render(state);
    assert(
      root.textContent.includes("-9223372036854775808") &&
        root.textContent.includes("1.25"),
      "typed values omitted by DOM",
    );
    assert(
      root.textContent.includes("Text unavailable") &&
        root.textContent.includes("Text redacted"),
      "text availability collapsed",
    );
    assert(
      !root.querySelector("img,script,iframe"),
      "typed value became markup",
    );
    const checkbox = root.querySelector("input[type=checkbox]");
    assert(
      checkbox.indeterminate && checkbox.disabled,
      "unknown/read-only checkbox collapsed",
    );
    const name = checkbox.getAttribute("aria-labelledby");
    assert(
      document.getElementById(name)?.textContent === "Name",
      "label relationship unresolved",
    );
    view.dispose();
    root.remove();
  } finally {
    receiver.close();
    assert(receiver.memoryUsed === 0, "contract leaked");
    receiver.free();
  }
  const profiles = [
    "mirror2d.core.v1",
    "semantics.core.v1",
    "content.no-raster.v1",
    "glyph.outline-single.v1",
    "text.clock.gtk-editable4224.v1",
  ];
  const limits = [
    1 << 20,
    32 << 10,
    16 << 20,
    64 << 20,
    16 << 20,
    128 << 20,
    8 << 20,
    100000,
    128,
  ];
  const hello = packet(
    2,
    join(
      scalar(1, 1),
      ...profiles.map((p) => blob(2, p)),
      blob(3, join(...limits.map((v, i) => scalar(i + 1, v)))),
      blob(4, join(scalar(1, 1), scalar(2, 1), blob(3, "Activate"))),
      blob(5, ids[2]),
    ),
  );
  const canvas = document.createElement("canvas");
  document.body.append(canvas);
  let callbacks,
    admitted = [],
    credentialResolve;
  const credential = new Promise((resolve) => (credentialResolve = resolve));
  const renderer = { render() {}, reset() {}, dispose() {} };
  const client = await BrowserClient.create({
    url: "ws://localhost",
    allowInsecure: true,
    clientId: ids[2],
    credential: () => credential,
    selection: "owned",
    canvas,
    renderer,
    transport: (url, cbs) => {
      callbacks = cbs;
      queueMicrotask(() => {
        cbs.opened();
        cbs.message(hello.buffer);
      });
      return {
        bufferedAmount: 0,
        send(bytes) {
          admitted.push(bytes.slice());
        },
        close() {},
      };
    },
  });
  const first = client.connect();
  let rejected = false;
  try {
    await client.connect();
  } catch {
    rejected = true;
  }
  assert(
    rejected && client.connectionState === "connecting",
    "concurrent credential attempts admitted",
  );
  credentialResolve(new Uint8Array(32).fill(7));
  await first;
  assert(client.connectionState === "connected", "connection state missing");
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", payload));
  let laterCalled = false;
  client.subscribe((event) => {
    if (event.type === "applied") {
      const copy = client.state;
      assert(
        client.memoryPeak >= client.memoryUsed && client.memoryUsed > 0,
        "Rust ownership high-water missing",
      );
      copy.records.clear();
      assert(
        client.state.records.size > 0,
        "consumer mutated the retained export",
      );
      client.dispose();
    }
  });
  client.subscribe((event) => {
    if (event.type === "applied") laterCalled = true;
  });
  const publication = new Uint8Array(16).fill(9);
  const meta = join(
    blob(1, publication),
    scalar(3, 0xffffffffffffffffn),
    scalar(4, payload.length),
    scalar(5, 1),
    scalar(
      6,
      fixture.state.records ? Object.keys(fixture.state.records).length : 0,
    ),
    blob(7, digest),
  );
  callbacks.message(packet(10, meta).buffer);
  callbacks.message(
    packet(12, join(blob(1, publication), scalar(2, 0), blob(3, payload)))
      .buffer,
  );
  callbacks.message(
    packet(
      13,
      join(
        blob(1, publication),
        scalar(2, payload.length),
        scalar(3, 1),
        scalar(4, Object.keys(fixture.state.records).length),
        blob(5, digest),
      ),
    ).buffer,
  );
  assert(
    client.connectionState === "disposed" && !laterCalled,
    "event disposal did not stop delivery",
  );
  const count = admitted.length;
  client.disconnect();
  client.disconnect();
  client.dispose();
  assert(
    client.connectionState === "disposed",
    "terminal disconnect changed disposed state",
  );
  assert(
    client.memoryPeak === 0 && client.memoryUsed === 0,
    "disposed status accessed retained WASM ownership",
  );
  callbacks.opened();
  callbacks.message(hello.buffer);
  assert(admitted.length === count, "stale transport callback emitted bytes");
  canvas.remove();
  await semanticSession();
  return {
    completeModel: true,
    maximumWidthIntegers: true,
    independentPythonOracle: true,
    concurrentConnect: true,
    listenerDisposal: true,
    generationFencing: true,
    ownedStateCopies: true,
    semanticReady: true,
    semanticNeverPresented: true,
  };
}

async function semanticSession() {
  const session = new Session(ids[2], "owned", true);
  session.setRequestNamespace(new Uint8Array(16).fill(9));
  const pick = (bytes, wanted) => {
    let at = 0;
    const u = () => {
      let out = 0n,
        shift = 0n;
      while (at < bytes.length) {
        const b = bytes[at++];
        out |= BigInt(b & 127) << shift;
        if (!(b & 128)) return out;
        shift += 7n;
      }
      throw Error("truncated");
    };
    while (at < bytes.length) {
      const key = Number(u()),
        tag = key >> 3,
        type = key & 7;
      let value;
      if (type === 0) value = u();
      else if (type === 2) {
        const length = Number(u());
        value = bytes.slice(at, at + length);
        at += length;
      } else throw Error("test field");
      if (tag === wanted) return value;
    }
  };
  const drain = () => {
    const frames = [];
    let frame;
    while ((frame = session.nextOutbound())) frames.push(frame);
    return frames;
  };
  const throws = (fn) => {
    let threw = false;
    try {
      fn();
    } catch {
      threw = true;
    }
    assert(threw, "unsupported semantic operation admitted");
  };
  try {
    session.start(new Uint8Array(32).fill(7));
    const hello = drain()[0];
    const encoded = new TextDecoder().decode(hello);
    assert(
      encoded.includes("semantics.only.v1") &&
        !encoded.includes("glyph.outline"),
      "semantic hello changed contract",
    );
    const profiles = [
      "mirror2d.core.v1",
      "semantics.core.v1",
      "content.no-raster.v1",
      "semantics.only.v1",
    ];
    const limits = [
      1 << 20,
      32 << 10,
      16 << 20,
      64 << 20,
      16 << 20,
      128 << 20,
      8 << 20,
      100000,
      128,
    ];
    session.receive(
      packet(
        2,
        join(
          scalar(1, 1),
          ...profiles.map((p) => blob(2, p)),
          blob(3, join(...limits.map((v, i) => scalar(i + 1, v)))),
          blob(
            4,
            join(
              scalar(1, 1),
              scalar(2, 1),
              blob(3, "Activate"),
              blob(3, "SetValue"),
            ),
          ),
          blob(5, ids[2]),
        ),
      ),
    );
    const open = drain()[0];
    assert(
      new TextDecoder().decode(open).includes("semantic-only"),
      "semantic OpenView absent",
    );
    const node = blob(
      3,
      join(
        scalar(1, 1),
        scalar(2, 1),
        scalar(3, 3),
        blob(10, join(blob(1, "Activate"), scalar(2, 1))),
        scalar(16, 1),
        scalar(17, 1),
      ),
    );
    const checkbox = blob(
      3,
      join(
        scalar(1, 2),
        scalar(2, 1),
        scalar(3, 4),
        blob(6, scalar(2, 0)),
        blob(10, join(blob(1, "SetValue"), scalar(2, 1))),
        scalar(16, 1),
        scalar(17, 1),
      ),
    );
    const payload = join(scalar(1, 1), blob(2, node), blob(2, checkbox)),
      pub = new Uint8Array(16).fill(12);
    const digest = new Uint8Array(
      await crypto.subtle.digest("SHA-256", payload),
    );
    session.receive(
      packet(
        10,
        join(
          blob(1, pub),
          scalar(3, 1),
          scalar(4, payload.length),
          scalar(5, 1),
          scalar(6, 2),
          blob(7, digest),
        ),
      ),
    );
    session.receive(packet(12, join(blob(1, pub), blob(3, payload))));
    session.receive(
      packet(
        13,
        join(
          blob(1, pub),
          scalar(2, payload.length),
          scalar(3, 1),
          scalar(4, 2),
          blob(5, digest),
        ),
      ),
    );
    assert(
      drain().every((bytes) => Number(pick(bytes, 5)) === 30),
      "Applied differed",
    );
    throws(() => session.acquire(0n));
    throws(() => session.presented(1n));
    throws(() => session.semanticReady(2n));
    session.semanticReady(1n);
    assert(Number(pick(drain()[0], 5)) === 35, "SemanticReady absent");
    session.acquire(0n);
    const acquire = drain()[0],
      request = pick(pick(acquire, 6), 1);
    session.receive(
      packet(
        41,
        join(
          blob(1, new Uint8Array(16).fill(8)),
          scalar(2, 1),
          scalar(3, 5000),
          blob(4, "Activate"),
          blob(4, "SetValue"),
          blob(5, request),
        ),
      ),
    );
    assert(session.hasControl, "semantic lease unavailable");
    assert(session.resourcesReady, "resource-free semantic graph not ready");
    assert(
      session.resourceCacheLost().type === "none" && session.hasControl,
      "resource-free cache loss disturbed readiness/authority",
    );
    assert(
      !session.evictUnpinnedResource(),
      "resource-free eviction invented a body",
    );
    throws(() => session.hitTest(0, 0, 10, 10, 0, 0, 1));
    const committed = session.state();
    const inventory = new BigUint64Array(
      [...committed.records].flatMap(([id, record]) => [
        id,
        Object.values(record)[0].version,
      ]),
    );
    assert(
      session.stateChanges(inventory).records.size === 0,
      "unchanged export copied records",
    );
    inventory[1] = 0n;
    const changed = session.stateChanges(inventory);
    assert(
      changed.records.size === 1 && changed.records.has(inventory[0]),
      "export omitted changed record",
    );
    assert(
      session.stateChanges(new BigUint64Array()).records.size ===
        committed.records.size,
      "empty inventory did not export full state",
    );
    throws(() => session.stateChanges(new BigUint64Array([1n])));
    const seq = session.activate(1n);
    assert(seq === 1n, "command sequence changed");
    assert(Number(pick(drain()[0], 5)) === 50, "semantic command absent");
    assert(
      [...session.state().records.values()].every((r) => "Semantic" in r),
      "visual record required",
    );
    session.receive(
      packet(54, join(scalar(1, seq), blob(2, "Applied"), scalar(5, 1))),
    );
    throws(() => session.setChecked(1n, true));
    const valueSeq = session.setChecked(2n, true);
    assert(valueSeq === 2n, "value command sequence changed");
    const originalValue = drain()[0];
    const valueCommand = pick(originalValue, 6);
    assert(pick(valueCommand, 7) === 2n, "value target changed");
    assert(pick(pick(valueCommand, 13), 1) === 1n, "boolean arguments changed");
    session.receive(
      packet(54, join(scalar(1, valueSeq), blob(2, "OutcomeUnknown"))),
    );
    assert(
      session.pendingCommands === 1,
      "WASM retired unknown original command",
    );
    throws(() => session.activate(1n));
    session.retryPending();
    assert(
      [...drain()[0]].every((byte, index) => byte === originalValue[index]),
      "WASM ambiguity retry changed original bytes",
    );
    session.disconnect();
    assert(!session.hasControl, "disconnect retained lease");
    throws(() => session.semanticReady(1n));
  } finally {
    session.close();
    assert(session.memoryUsed === 0, "semantic reservations leaked");
    session.free();
  }
}
