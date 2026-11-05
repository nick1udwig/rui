import {
  BrowserClient,
  CanvasRenderer,
  type State,
  type Viewport,
  type Digest,
  type TransportFactory,
} from "@rui/client-browser";
import "./style.css";
const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `<main><header><p class="eyebrow">RUI / VECTOR ARCADE</p><h1 id="title">Geometry across the wire.</h1><p>A native desktop game, controlled here. Every line is drawn from the host’s RUI scene.</p></header><section class="toolbar"><label>Connection code <input id="code" type="text" placeholder="ABCD-1234-EF56" autocomplete="off" autocapitalize="characters" spellcheck="false" maxlength="32"></label><button id="connect">Connect</button><button id="control" disabled>Take control</button><button id="release" disabled>Release control</button><label>Render scale <select id="zoom"><option value="0.5">½×</option><option value="0.75" selected>¾×</option><option value="1">1×</option><option value="1.5">1½×</option><option value="2">2×</option></select></label></section><p id="status" role="status">Scan the host QR code, or enter its connection code.</p><section class="cabinet"><div class="viewport"><canvas id="screen" aria-label="Remote vector game"></canvas></div></section><section id="controls" aria-label="Game controls"></section><section class="metrics"><div><strong id="fps">0</strong><span>presented frames / s</span></div><div><strong id="bandwidth">0</strong><span>KiB / s received</span></div><div><strong id="draw">0</strong><span>ms draw / frame</span></div><div><strong id="lines">0</strong><span>retained line slots</span></div></section><footer><p id="keyboard">← → steer · Space fire · ↑ thrust · H hyperspace · C coin · Enter start</p><p>Host simulation · RUI Protobuf · vector scene updates · local rasterization</p><p>Fixed-width beam presentation. CRT glow and remote audio are outside this demo.</p></footer></main>`;
const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const canvas = element<HTMLCanvasElement>("screen");
canvas.tabIndex = 0;
canvas.onclick = () => canvas.focus();
const status = element("status");
const codeInput = element<HTMLInputElement>("code");
const invitation = new URLSearchParams(location.hash.slice(1)).get("code");
if (location.hash)
  history.replaceState(null, "", location.pathname + location.search);
if (invitation) codeInput.value = invitation;
let savedCredential: string | undefined;
try {
  savedCredential = sessionStorage.getItem("rui.game.credential") ?? undefined;
} catch {}

const connect = element<HTMLButtonElement>("connect");
const control = element<HTMLButtonElement>("control");
const release = element<HTMLButtonElement>("release");
const nativeRenderer = new CanvasRenderer(canvas);
let zoom = innerWidth < 600 ? 0.5 : 0.75;
element<HTMLSelectElement>("zoom").value = String(zoom);
const renderer = {
  render(
    state: State,
    _viewport: Viewport,
    resources: (digest: Digest) => Uint8Array,
  ) {
    nativeRenderer.render(
      state,
      {
        width: Math.ceil(640 * zoom),
        height: Math.ceil(480 * zoom),
        origin: { x: 0, y: 0 },
        scale: zoom,
      },
      resources,
    );
  },
  reset: () => nativeRenderer.reset(),
  dispose: () => nativeRenderer.dispose(),
};
element<HTMLSelectElement>("zoom").onchange = (event) => {
  zoom = Number((event.target as HTMLSelectElement).value);
};
let totalIncoming = 0,
  totalPresented = 0;
let incoming = 0,
  outgoing = 0,
  frames = 0,
  renderMs = 0,
  lastMetric = performance.now();
const transport: TransportFactory = (url, callbacks) => {
  const ws = new WebSocket(url, "rui.v1");
  ws.binaryType = "arraybuffer";
  ws.onopen = () => {
    if (ws.protocol === "rui.v1") callbacks.opened();
    else {
      ws.close();
      callbacks.failed();
    }
  };
  ws.onmessage = (event) => {
    if (!(event.data instanceof ArrayBuffer)) {
      ws.close();
      callbacks.failed();
      return;
    }
    incoming += event.data.byteLength;
    totalIncoming += event.data.byteLength;
    callbacks.message(event.data);
  };
  ws.onclose = () => callbacks.closed();
  ws.onerror = () => callbacks.failed();
  return {
    get bufferedAmount() {
      return ws.bufferedAmount;
    },
    send(bytes) {
      outgoing += bytes.byteLength;
      ws.send(bytes as Uint8Array<ArrayBuffer>);
    },
    close() {
      ws.close();
    },
  };
};
let client: BrowserClient | undefined;
let controlling = false;
let game = "asteroid";
const keyState = new Set<string>();
const pointerState = new Set<bigint>();
const pulseTimers = new Map<bigint, ReturnType<typeof setTimeout>>();
const pressedAt = new Map<bigint, number>();
const wanted = new Map<bigint, boolean>();
const sent = new Map<bigint, boolean>();
let held = false;
let lastHeartbeat = 0;
const buttons = new Map<bigint, HTMLButtonElement>();
let targets: bigint[] = [];
let commandStarted = 0;
const inputTimes: number[] = [];
function set(target: bigint, value: boolean) {
  if (!controlling) return;
  wanted.set(target, value);
  buttons.get(target)?.classList.toggle("held", value);
  flush();
}
function flush() {
  if (!client?.hasControl || client.pendingCommands) return;
  const changed = [...wanted]
    .filter(([id, value]) => sent.get(id) !== value)
    .sort((a, b) => Number(a[1]) - Number(b[1]));
  const update =
    changed[0] ??
    (performance.now() - lastHeartbeat > 100
      ? [...wanted].find(([, value]) => value)
      : undefined);
  if (!update) return;
  try {
    client.setChecked(update[0], update[1]);
    sent.set(update[0], update[1]);
    commandStarted = performance.now();
    lastHeartbeat = performance.now();
  } catch (error) {
    status.textContent = String(error);
    /* Wait for current presentation/readiness before retrying. */
  }
}
function clear() {
  keyState.clear();
  pointerState.clear();
  pressedAt.clear();
  for (const timer of pulseTimers.values()) clearTimeout(timer);
  pulseTimers.clear();
  wanted.clear();
  sent.clear();
  held = false;
  for (const button of buttons.values()) button.classList.remove("held");
}
function relinquish() {
  clear();
  if (controlling) client?.releaseControl();
  controlling = false;
}
connect.onclick = async () => {
  connect.disabled = true;
  try {
    if (codeInput.value.trim()) {
      const response = await fetch("/api/game/pair", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ code: codeInput.value }),
        cache: "no-store",
        credentials: "omit",
        referrerPolicy: "no-referrer",
      });
      const result = await response.json();
      if (!response.ok) throw Error(result.error ?? "Connection code rejected");
      if (
        typeof result.credential !== "string" ||
        !/^[a-f0-9]{64}$/.test(result.credential)
      )
        throw Error("Invalid host authorization");
      savedCredential = result.credential;
      try {
        sessionStorage.setItem("rui.game.credential", savedCredential!);
      } catch {}
      codeInput.value = "";
    }
    if (!savedCredential || !/^[a-f0-9]{64}$/.test(savedCredential))
      throw Error("Enter the connection code printed by the host.");
    const credential = Uint8Array.from(
      savedCredential.match(/.{2}/g)!,
      (byte) => parseInt(byte, 16),
    );
    client?.dispose();
    client = await BrowserClient.create({
      url: `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/rui`,
      allowInsecure: location.protocol === "http:",
      clientId: new Uint8Array(16).fill(3),
      selection: "game",
      canvas,
      renderer,
      transport,
      credential: async () => credential.slice(),
      onEvent(event) {
        if (event.type === "connected") {
          status.textContent = "Connected as observer. Take control to play.";
          control.disabled = false;
        }
        if (event.type === "control") {
          controlling = event.granted;
          for (const button of buttons.values())
            button.disabled = !event.granted;
          control.disabled = event.granted;
          release.disabled = !event.granted;
          if (!event.granted) clear();
          status.textContent = event.granted
            ? "You control the desktop game."
            : "Observer. Take control to play.";
        }
        if (event.type === "presented") {
          frames++;
          totalPresented++;
          renderMs = event.renderMs;
          const nodes = [...client!.state!.records.values()].flatMap(
            (record) =>
              "Semantic" in record &&
              record.Semantic.actions.some((a) => a.id === "SetValue")
                ? [record.Semantic]
                : [],
          );
          if (!buttons.size) {
            game = nodes[0]?.label?.includes("track") ? "bzone" : "asteroid";
            document.body.classList.toggle("battlezone", game === "bzone");
            element("title").textContent =
              game === "bzone"
                ? "Battlezone. Across the wire."
                : "Asteroids. Across the wire.";
            element("keyboard").textContent =
              game === "bzone"
                ? "C twice, then Enter to start · ↑ ↓ drive · ← → turn · Space fire · W/S left track · I/K right track"
                : "← → steer · Space fire · ↑ thrust · H hyperspace · C coin · Enter start";
            targets = nodes.map((n) => n.id);
            for (const node of nodes) {
              const button = document.createElement("button");
              button.textContent = node.label ?? "Control";
              button.disabled = !controlling;
              button.onpointerdown = (event) => {
                event.preventDefault();
                button.setPointerCapture(event.pointerId);
                pointerState.add(node.id);
                press(node.id);
              };
              button.onpointerup = () => {
                pointerState.delete(node.id);
                releaseButton(node.id);
              };
              button.onpointercancel = () => {
                pointerState.delete(node.id);
                releaseButton(node.id);
              };
              button.onlostpointercapture = () => {
                pointerState.delete(node.id);
                releaseButton(node.id);
              };
              button.onkeydown = (event) => {
                if (event.key === " " || event.key === "Enter") {
                  event.preventDefault();
                  if (!event.repeat) press(node.id);
                }
              };
              button.onkeyup = (event) => {
                if (event.key === " " || event.key === "Enter") {
                  event.preventDefault();
                  releaseButton(node.id);
                }
              };
              buttons.set(node.id, button);
              element("controls").append(button);
            }
          }
          for (const node of nodes) {
            const value =
              node.value && "Boolean" in node.value
                ? node.value.Boolean
                : false;
            if (!wanted.has(node.id)) sent.set(node.id, value);
          }
          element("lines").textContent = String(
            [...client!.state!.records.values()].filter((r) => "Scene" in r)
              .length - 2,
          );
          flush();
        }
        if (event.type === "commandResult") {
          if (event.status === "Dispatched") {
            inputTimes.push(performance.now() - commandStarted);
            if (inputTimes.length > 1024) inputTimes.shift();
          }
          if (
            controlling &&
            (event.status === "Dispatched" || event.status === "Applied")
          )
            status.textContent = "You control the desktop game.";
          if (event.status !== "Dispatched" && event.status !== "Applied") {
            sent.clear();
            status.textContent = `Input: ${event.status}`;
          }
          flush();
        }
        if (event.type === "error") status.textContent = `Host: ${event.code}`;
        if (event.type === "failure") {
          controlling = false;
          clear();
          status.textContent = event.message;
          connect.disabled = false;
        }
        if (event.type === "disconnected") {
          controlling = false;
          clear();
          connect.disabled = false;
          control.disabled = true;
          release.disabled = true;
        }
      },
    });
    await client.connect();
  } catch (error) {
    status.textContent = String(error);
    connect.disabled = false;
  }
};
control.onclick = () => {
  client?.requestControl();
  canvas.focus();
};
release.onclick = relinquish;
function press(id: bigint) {
  const timer = pulseTimers.get(id);
  if (timer) clearTimeout(timer);
  pulseTimers.delete(id);
  pressedAt.set(id, performance.now());
  set(id, true);
}
function releaseButton(id: bigint) {
  if (targets.indexOf(id) >= 5) {
    const remaining = 180 - (performance.now() - (pressedAt.get(id) ?? 0));
    if (remaining > 0) {
      if (!pulseTimers.has(id))
        pulseTimers.set(
          id,
          setTimeout(() => {
            pulseTimers.delete(id);
            set(id, false);
          }, remaining),
        );
      return;
    }
  }
  set(id, false);
}
const keys: Record<string, number> = {
  ArrowLeft: 0,
  ArrowRight: 1,
  Space: 2,
  ArrowUp: 3,
  KeyH: 4,
  KeyC: 5,
  Enter: 6,
};
function keyboardValues(): boolean[] {
  const has = (...codes: string[]) => codes.some((code) => keyState.has(code));
  if (game === "bzone") {
    const drive = Number(has("ArrowUp")) - Number(has("ArrowDown"));
    const turn = Number(has("ArrowRight")) - Number(has("ArrowLeft"));
    const left = drive + turn + Number(has("KeyW")) - Number(has("KeyS"));
    const right = drive - turn + Number(has("KeyI")) - Number(has("KeyK"));
    return [
      left > 0,
      left < 0,
      has("Space"),
      right > 0,
      right < 0,
      has("KeyC"),
      has("Enter"),
    ];
  }
  return targets.map((_, i) =>
    Object.entries(keys).some(
      ([key, index]) => index === i && keyState.has(key),
    ),
  );
}
for (const type of ["keydown", "keyup"] as const)
  window.addEventListener(type, (event) => {
    if (
      event.target instanceof HTMLInputElement ||
      event.target instanceof HTMLSelectElement ||
      event.target instanceof HTMLButtonElement ||
      !controlling
    )
      return;
    const supported =
      game === "bzone"
        ? [
            "ArrowLeft",
            "ArrowRight",
            "ArrowUp",
            "ArrowDown",
            "Space",
            "KeyC",
            "Enter",
            "KeyW",
            "KeyS",
            "KeyI",
            "KeyK",
          ].includes(event.code)
        : event.code in keys;
    if (!supported) return;
    event.preventDefault();
    if (event.repeat) return;
    if (type === "keydown") keyState.add(event.code);
    else keyState.delete(event.code);
    const values = keyboardValues();
    targets.forEach((id, i) => {
      const value = values[i] || pointerState.has(id);
      if (value && !wanted.get(id)) press(id);
      else if (!value && wanted.get(id)) releaseButton(id);
    });
    held = keyState.size > 0;
  });
window.addEventListener("blur", relinquish);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) relinquish();
});
window.addEventListener("pagehide", () => client?.disconnect());
setInterval(() => {
  flush();
  const now = performance.now(),
    seconds = (now - lastMetric) / 1000;
  if (seconds >= 1) {
    element("fps").textContent = (frames / seconds).toFixed(1);
    element("bandwidth").textContent = (incoming / 1024 / seconds).toFixed(1);
    element("draw").textContent = renderMs.toFixed(1);
    frames = 0;
    incoming = 0;
    lastMetric = now;
  }
}, 20);
Object.defineProperty(window, "ruiGame", {
  value: () => ({
    revision: String(client?.state?.revision ?? 0),
    controller: controlling,
    ready: client?.hasControl ?? false,
    pending: client?.pendingCommands ?? 0,
    inputTimes: [...inputTimes],
    receivedBytes: totalIncoming,
    presentedFrames: totalPresented,
    game,
    outgoing,
    targets: targets.map(String),
    held,
  }),
});

codeInput.addEventListener("keydown", (event) => {
  if (event.key === "Enter") connect.click();
});
if (invitation) connect.click();
