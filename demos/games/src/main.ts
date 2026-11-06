import {
  BrowserClient,
  CanvasRenderer,
  type State,
  type Viewport,
  type Digest,
  type TransportFactory,
} from "@rui/client-browser";
import "./style.css";
const icon = (body: string) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
const icons = {
  up: icon('<path d="M6 15l6-6 6 6"/>'),
  down: icon('<path d="M6 9l6 6 6-6"/>'),
  left: icon('<path d="M15 6l-6 6 6 6"/>'),
  right: icon('<path d="M9 6l6 6-6 6"/>'),
  thrust: icon('<path d="M6 12l6-6 6 6M6 18l6-6 6 6"/>'),
  fire: icon(
    '<circle cx="12" cy="12" r="7"/><path d="M12 2v4M12 18v4M2 12h4M18 12h4"/><circle cx="12" cy="12" r="1" fill="currentColor"/>',
  ),
  hyper: icon(
    '<circle cx="12" cy="12" r="8" stroke-dasharray="2.5 3"/><circle cx="12" cy="12" r="2.5"/>',
  ),
  coin: icon('<circle cx="12" cy="12" r="8"/><path d="M12 8v8"/>'),
  start: icon('<path d="M8 5.5v13l10-6.5z"/>'),
};
type Key = { area: string; icon: keyof typeof icons; text: string };
const layouts: Record<string, { keys: Key[]; captions: [string, string][] }> = {
  bzone: {
    keys: [
      { area: "lf", icon: "up", text: "Forward" },
      { area: "lr", icon: "down", text: "Reverse" },
      { area: "fire", icon: "fire", text: "Fire" },
      { area: "rf", icon: "up", text: "Forward" },
      { area: "rr", icon: "down", text: "Reverse" },
      { area: "coin", icon: "coin", text: "Coin" },
      { area: "start", icon: "start", text: "Start" },
    ],
    captions: [
      ["lc", "Left track"],
      ["rc", "Right track"],
    ],
  },
  asteroid: {
    keys: [
      { area: "left", icon: "left", text: "Left" },
      { area: "right", icon: "right", text: "Right" },
      { area: "fire", icon: "fire", text: "Fire" },
      { area: "thrust", icon: "thrust", text: "Thrust" },
      { area: "hyper", icon: "hyper", text: "Hyperspace" },
      { area: "coin", icon: "coin", text: "Coin" },
      { area: "start", icon: "start", text: "Start" },
    ],
    captions: [],
  },
};
const keyHelp: Record<string, [string[], string][]> = {
  bzone: [
    [["C"], "Coin, twice"],
    [["Enter"], "Start"],
    [["↑", "↓"], "Drive"],
    [["←", "→"], "Turn"],
    [["Space"], "Fire"],
    [["W", "S"], "Left track"],
    [["I", "K"], "Right track"],
  ],
  asteroid: [
    [["C"], "Coin"],
    [["Enter"], "Start"],
    [["←", "→"], "Rotate"],
    [["↑"], "Thrust"],
    [["Space"], "Fire"],
    [["H"], "Hyperspace"],
  ],
};
const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `<div class="app"><header class="topbar"><div class="brand"><svg class="mark" viewBox="0 0 28 28" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" aria-hidden="true"><path d="M3 23h22M14 4 6 23M14 4l8 19M14 4v19"/></svg><div class="brand-text"><span class="brand-name">RUI ARCADE</span><h1 id="title">Vector arcade</h1></div></div><span class="pill" id="pill">Offline</span></header><div class="layout"><section class="stage"><div class="bezel"><div class="screen"><canvas id="screen" aria-label="Remote vector game"></canvas><div class="overlay"><form class="connect" id="connect-form"><h2>Connect to the host</h2><p>Scan the QR code in the host terminal, or enter its connection code.</p><div class="field"><input id="code" type="text" placeholder="ABCD-1234-EF56" aria-label="Connection code" autocomplete="off" autocapitalize="characters" spellcheck="false" maxlength="32"><button class="btn primary" id="connect" type="submit">Connect</button></div><p class="hint" id="hint"></p></form></div></div></div><div class="session"><p id="status" role="status">Not connected.</p><label class="select">Resolution <select id="zoom"><option value="0.5">320×240</option><option value="0.75">480×360</option><option value="1">640×480</option><option value="1.5">960×720</option><option value="2">1280×960</option></select></label><button class="btn" id="release" disabled>Release</button><button class="btn primary" id="control" disabled>Take control</button></div></section><aside class="deck"><section class="panel"><h2 class="panel-title"><span>Controls</span></h2><div id="controls" aria-label="Game controls"><p class="placeholder">Controls appear when the game streams.</p></div></section><section class="panel"><h2 class="panel-title"><span>Telemetry</span><span>RUI bytes, excl. TLS</span></h2><div class="metrics"><div><strong id="fps">0</strong><span>frames / s</span></div><div><strong id="bandwidth">0</strong><span>KiB / s in</span></div><div><strong id="draw">0</strong><span>ms draw</span></div><div><strong id="lines">0</strong><span>line slots</span></div></div></section><section class="panel keys-panel" id="keys-panel" hidden><h2 class="panel-title"><span>Keyboard</span><span>click screen to focus</span></h2><dl class="keys" id="keys"></dl></section><p class="note">MAME runs on the host. The browser receives the RUI vector scene and rasterizes it locally with fixed-width beams. CRT glow and audio are not sent.</p></aside></div></div>`;
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
const pill = element("pill");
const hint = element("hint");
const stateLabels = {
  offline: "Offline",
  connecting: "Connecting",
  observing: "Observing",
  control: "In control",
};
function setState(state: keyof typeof stateLabels) {
  document.body.dataset.state = state;
  pill.textContent = stateLabels[state];
}
function setStatus(text: string, error = false) {
  status.textContent = text;
  hint.textContent = text;
  hint.classList.toggle("error", error);
}
setState("offline");
if (savedCredential)
  hint.textContent = "Saved authorization found. Connect, or enter a new code.";
const nativeRenderer = new CanvasRenderer(canvas);
const scales = [0.5, 0.75, 1, 1.5, 2];
const fitted = canvas.getBoundingClientRect().width * devicePixelRatio;
let zoom = Math.min(
  scales.find((scale) => 640 * scale >= fitted * 0.9) ?? 2,
  matchMedia("(hover: none)").matches ? 1 : 1.5,
);
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
    setStatus(String(error), true);
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
  if (controlling) {
    client?.releaseControl();
    setState("observing");
  }
  controlling = false;
}
element<HTMLFormElement>("connect-form").onsubmit = (event) =>
  event.preventDefault();
connect.onclick = async () => {
  connect.disabled = true;
  setState("connecting");
  setStatus("Connecting…");
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
          setState("observing");
          setStatus("Observing. Take control to play.");
          control.disabled = false;
        }
        if (event.type === "control") {
          controlling = event.granted;
          for (const button of buttons.values())
            button.disabled = !event.granted;
          control.disabled = event.granted;
          release.disabled = !event.granted;
          if (!event.granted) clear();
          setState(event.granted ? "control" : "observing");
          setStatus(
            event.granted
              ? "You control the desktop game."
              : "Observing. Take control to play.",
          );
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
            const layout = layouts[game];
            const title = game === "bzone" ? "Battlezone" : "Asteroids";
            document.body.classList.add(game === "bzone" ? "battlezone" : "asteroid");
            element("title").textContent = title;
            document.title = `${title} · RUI arcade`;
            element("keys").innerHTML = keyHelp[game]
              .map(
                ([keys, action]) =>
                  `<dt>${keys.map((key) => `<kbd>${key}</kbd>`).join("")}</dt><dd>${action}</dd>`,
              )
              .join("");
            element("keys-panel").hidden = false;
            const controlsElement = element("controls");
            controlsElement.replaceChildren();
            for (const [area, text] of layout.captions) {
              const caption = document.createElement("span");
              caption.className = "caption";
              caption.style.gridArea = area;
              caption.textContent = text;
              controlsElement.append(caption);
            }
            targets = nodes.map((n) => n.id);
            nodes.forEach((node, index) => {
              const key = layout.keys[index];
              const button = document.createElement("button");
              const label = node.label ?? "Control";
              button.setAttribute("aria-label", label);
              button.title = label;
              if (key) {
                button.className = `key ${key.area}`;
                button.classList.toggle("fire", key.area === "fire");
                button.classList.toggle("small", index >= 5);
                button.style.gridArea = key.area;
                button.innerHTML = `${icons[key.icon]}<span>${key.text}</span>`;
              } else {
                button.className = "key";
                button.textContent = label;
              }
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
              controlsElement.append(button);
            });
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
            setStatus("You control the desktop game.");
          if (event.status !== "Dispatched" && event.status !== "Applied") {
            sent.clear();
            setStatus(`Input: ${event.status}`, true);
          }
          flush();
        }
        if (event.type === "error") setStatus(`Host: ${event.code}`, true);
        if (event.type === "failure") {
          controlling = false;
          clear();
          setState("offline");
          setStatus(event.message, true);
          connect.disabled = false;
        }
        if (event.type === "disconnected") {
          if (document.body.dataset.state !== "offline")
            setStatus("Disconnected from the host.");
          setState("offline");
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
    setState("offline");
    setStatus(error instanceof Error ? error.message : String(error), true);
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

if (invitation) connect.click();
