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
app.innerHTML = `<main><header><p class="eyebrow">RUI / VECTOR ARCADE</p><h1>Geometry across the wire.</h1><p>A native desktop game, controlled here. Every line is drawn from the host’s RUI scene.</p></header><section class="toolbar"><label>Host credential <input id="credential" type="file"></label><button id="connect">Connect</button><button id="control" disabled>Take control</button><button id="release" disabled>Release control</button><label>Render scale <select id="zoom"><option value="0.5">½×</option><option value="0.75" selected>¾×</option><option value="1">1×</option><option value="1.5">1½×</option><option value="2">2×</option></select></label></section><p id="status" role="status">Choose the credential printed by the host.</p><section class="cabinet"><div class="viewport"><canvas id="screen" aria-label="Remote vector game"></canvas></div></section><section id="controls" aria-label="Game controls"></section><section class="metrics"><div><strong id="fps">0</strong><span>presented frames / s</span></div><div><strong id="bandwidth">0</strong><span>KiB / s received</span></div><div><strong id="draw">0</strong><span>ms draw / frame</span></div><div><strong id="lines">0</strong><span>retained line slots</span></div></section><footer><p>← → steer · Space fire · ↑ thrust · H hyperspace · C coin · Enter start</p><p>Host simulation · RUI Protobuf · vector scene updates · local rasterization</p><p>Fixed-width beam presentation. CRT glow and remote audio are outside this demo.</p></footer></main>`;
const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const canvas = element<HTMLCanvasElement>("screen");
canvas.tabIndex = 0;
canvas.onclick = () => canvas.focus();
const status = element("status");
const credential = element<HTMLInputElement>("credential");
const connect = element<HTMLButtonElement>("connect");
const control = element<HTMLButtonElement>("control");
const release = element<HTMLButtonElement>("release");
const nativeRenderer = new CanvasRenderer(canvas);
let zoom = 0.75;
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
  ws.onopen = () => callbacks.opened();
  ws.onmessage = (event) => {
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
  const file = credential.files?.[0];
  if (!file || file.size !== 32) {
    status.textContent = "Choose the host’s 32-byte credential.";
    return;
  }
  connect.disabled = true;
  try {
    client?.dispose();
    client = await BrowserClient.create({
      url: `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/rui`,
      allowInsecure: location.protocol === "http:",
      clientId: new Uint8Array(16).fill(3),
      selection: "game",
      canvas,
      renderer,
      transport,
      credential: async () => new Uint8Array(await file.arrayBuffer()),
      onEvent(event) {
        if (event.type === "connected") {
          status.textContent = "Connected as observer. Take control to play.";
          control.disabled = false;
        }
        if (event.type === "control") {
          controlling = event.granted;
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
            targets = nodes.map((n) => n.id);
            for (const node of nodes) {
              const button = document.createElement("button");
              button.textContent = node.label ?? "Control";
              button.disabled = false;
              button.onpointerdown = (event) => {
                event.preventDefault();
                button.setPointerCapture(event.pointerId);
                set(node.id, true);
              };
              button.onpointerup = () => set(node.id, false);
              button.onpointercancel = () => set(node.id, false);
              button.onlostpointercapture = () => set(node.id, false);
              button.onkeydown = (event) => {
                if (event.key === " " || event.key === "Enter") {
                  event.preventDefault();
                  if (!event.repeat) set(node.id, true);
                }
              };
              button.onkeyup = (event) => {
                if (event.key === " " || event.key === "Enter") {
                  event.preventDefault();
                  set(node.id, false);
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
          inputTimes.push(performance.now() - commandStarted);
          if (inputTimes.length > 1024) inputTimes.shift();
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
const keys: Record<string, number> = {
  ArrowLeft: 0,
  ArrowRight: 1,
  Space: 2,
  ArrowUp: 3,
  KeyH: 4,
  KeyC: 5,
  Enter: 6,
};
for (const type of ["keydown", "keyup"] as const)
  window.addEventListener(type, (event) => {
    if (
      event.target instanceof HTMLInputElement ||
      event.target instanceof HTMLSelectElement ||
      event.target instanceof HTMLButtonElement ||
      !controlling ||
      !(event.code in keys)
    )
      return;
    event.preventDefault();
    if (event.repeat) return;
    const id = targets[keys[event.code]];
    if (id !== undefined) set(id, type === "keydown");
    held = type === "keydown";
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
    outgoing,
    targets: targets.map(String),
    held,
  }),
});
