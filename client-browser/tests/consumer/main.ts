import {
  BrowserClient,
  CanvasRenderer,
  Timings,
  initialize,
  text,
  webSocketTransport,
  type BrowserEvent,
  type BrowserOptions,
  type Renderer,
  type TransportFactory,
} from "@rui/client-browser";

// This file is copied into a separate consumer before installing the tarball.
// It imports only the documented ESM entry point and never the generated actor.
let client: BrowserClient | undefined;
let painted = 0n;
let events: BrowserEvent[] = [];
let renders = 0,
  sends = 0,
  releases = 0;
let receivedBytes = 0,
  sentBytes = 0,
  receivedFrames = 0;
let currentRevision = 0n;
let timing: Timings | undefined;
let rendererStats: CanvasRenderer | undefined;
let activeMeasurement:
  | {
      started: number;
      received: number;
      sent: number;
      frames: number;
      finish(value: unknown): void;
      fail(error: unknown): void;
      timer: ReturnType<typeof setTimeout>;
    }
  | undefined;
let holdingResults = false;
let pageShows = 0,
  restoredFromCache = 0;
let heldFrames: ArrayBuffer[] = [];
let heldBytes = 0;
let reconnectBusyRetries = 0;
const producerBusy =
  "host is still closing the previous connection; reconnect shortly";
async function connectWithCloseRetry() {
  const deadline = performance.now() + 2000;
  for (;;) {
    try {
      await client!.connect();
      return;
    } catch (error) {
      if (
        !(error instanceof Error) ||
        error.message !== producerBusy ||
        performance.now() >= deadline
      )
        throw error;
      reconnectBusyRetries++;
      await new Promise((done) => setTimeout(done, 50));
    }
  }
}
function packetKind(bytes: ArrayBuffer): number {
  const data = new Uint8Array(bytes);
  let at = 0;
  const varint = () => {
    let value = 0n,
      shift = 0n;
    while (at < data.length && shift < 70n) {
      const b = data[at++];
      value |= BigInt(b & 127) << shift;
      if (!(b & 128)) return value;
      shift += 7n;
    }
    throw Error("fixture header");
  };
  while (at < data.length) {
    const tag = varint(),
      wire = Number(tag & 7n);
    if (wire === 0) {
      const value = varint();
      if (tag >> 3n === 5n) return Number(value);
    } else if (wire === 2) {
      const length = Number(varint());
      at += length;
    } else throw Error("fixture header");
  }
  return 0;
}
function completeMeasurement() {
  if (
    !activeMeasurement ||
    !client ||
    client.connectionState !== "connected" ||
    painted !== currentRevision ||
    painted === 0n ||
    !client.resourcesReady ||
    client.pendingCommands
  )
    return;
  const measurement = activeMeasurement;
  activeMeasurement = undefined;
  clearTimeout(measurement.timer);
  const durationMs = performance.now() - measurement.started;
  measurement.finish({
    durationMs,
    receivedBytes: receivedBytes - measurement.received,
    sentBytes: sentBytes - measurement.sent,
    receivedFrames: receivedFrames - measurement.frames,
    revision: painted.toString(),
    memoryUsed: client.memoryUsed,
    memoryPeak: client.memoryPeak,
    renderer: rendererStats?.allocationStats(),
    pending: client.pendingCommands,
    authority: client.hasControl,
    reconnectBusyRetries,
  });
}
const canvas = document.querySelector<HTMLCanvasElement>("#view")!;
const status = document.querySelector<HTMLElement>("#status")!;
function nodes() {
  return [...(client?.state?.records.values() ?? [])].flatMap((r) =>
    "Semantic" in r ? [r.Semantic] : [],
  );
}
function editable() {
  return nodes().find(
    (n) =>
      n.actions.some((a) => a.id === "ReplaceText" && a.schema_version === 1) &&
      n.text &&
      text(client!.state!, n.id)?.editable,
  );
}
function inspected() {
  const state = client?.state;
  return {
    connection: client?.connectionState,
    revision: state?.revision,
    presented: painted,
    hasControl: client?.hasControl,
    pending: client?.pendingCommands,
    resourcesReady: client?.resourcesReady,
    memoryUsed: client?.memoryUsed,
    memoryPeak: client?.memoryPeak,
    capabilities: client?.capabilities,
    renders,
    sends,
    releases,
    text: editable()?.text ? text(state!, editable()!.id)?.content : null,
    failures: events.filter((e) => e.type === "failure"),
    results: events.filter((e) => e.type === "commandResult"),
    events: events.slice(-12),
    state,
    pageShows,
    restoredFromCache,
  };
}
const consumer = {
  initialize: async (asset?: string) => {
    try {
      await initialize(asset);
      status.textContent = "WASM initialized";
      return true;
    } catch (error) {
      status.textContent = String(error);
      return false;
    }
  },
  create: async (
    url: string,
    credential: number[],
    replacement = false,
    clientId = Array(16).fill(3),
    measurements = false,
  ) => {
    client?.dispose();
    events = [];
    painted = 0n;
    currentRevision = 0n;
    renders = sends = releases = 0;
    receivedBytes = sentBytes = receivedFrames = 0;
    timing = measurements ? new Timings(16384) : undefined;
    const options: BrowserOptions = {
      url,
      credential: async () => new Uint8Array(credential),
      clientId: new Uint8Array(clientId),
      selection: "fixture",
      canvas,
      allowInsecure: url.startsWith("ws:"),
      scale: 1,
      timings: timing,
      onEvent: (event) => {
        events.push(event);
        if (events.length > 512) events.shift();
        if (event.type === "applied" || event.type === "resourcesNeeded")
          currentRevision = event.revision;
        if (event.type === "presented") painted = event.revision;
        if (
          event.type === "failure" &&
          activeMeasurement &&
          event.message !== producerBusy
        ) {
          clearTimeout(activeMeasurement.timer);
          activeMeasurement.fail(Error(event.message));
          activeMeasurement = undefined;
        }
        completeMeasurement();
      },
    };
    if (replacement || measurements) {
      const delegate = new CanvasRenderer(canvas);
      rendererStats = delegate;
      const renderer: Renderer = {
        render(state, viewport, resource) {
          renders++;
          return delegate.render(state, viewport, resource);
        },
        reset() {
          delegate.reset();
        },
        dispose() {
          releases++;
          delegate.dispose();
        },
      };
      const transport: TransportFactory = (address, callbacks) => {
        const delegate = webSocketTransport(address, {
          ...callbacks,
          message(bytes) {
            receivedBytes += bytes.byteLength;
            receivedFrames++;
            if (holdingResults && packetKind(bytes) === 54) {
              if (heldBytes + bytes.byteLength > 65536)
                throw Error("held result limit");
              heldFrames.push(bytes.slice(0));
              heldBytes += bytes.byteLength;
              return;
            }
            callbacks.message(bytes);
          },
        });
        return {
          get bufferedAmount() {
            return delegate.bufferedAmount;
          },
          send(bytes) {
            sends++;
            sentBytes += bytes.length;
            const owned = bytes.slice();
            try {
              delegate.send(owned);
            } finally {
              owned.fill(0);
            }
          },
          close() {
            for (const frame of heldFrames) new Uint8Array(frame).fill(0);
            heldFrames = [];
            heldBytes = 0;
            holdingResults = false;
            delegate.close();
          },
        };
      };
      options.renderer = renderer;
      options.transport = transport;
    }
    client = await BrowserClient.create(options);
    status.textContent = "Client created";
  },
  connect: () => connectWithCloseRetry(),
  inspect: inspected,
  requestControl: () => client!.requestControl(),
  releaseControl: () => client!.releaseControl(),
  activate: () => {
    const target = nodes().find(
      (n) =>
        n.role === "Button" &&
        n.actions.some((a) => a.id === "Activate" && a.schema_version === 1),
    );
    if (!target) throw Error("no advertised button");
    return client!.activate(target.id);
  },
  focusText: () => {
    const target = editable();
    if (!target) throw Error("no editable text");
    return client!.focus(target.id);
  },
  edit: (value: string) => {
    const target = editable();
    if (!target) throw Error("no editable text");
    return client!.commitEdit(client!.captureEdit(target.id), value);
  },
  disconnect: () => {
    painted = 0n;
    client!.disconnect();
  },
  dispose: () => client!.dispose(),
  diagnostics: () => ({
    memoryUsed: client?.memoryUsed,
    memoryPeak: client?.memoryPeak,
    renderer: rendererStats?.allocationStats(),
    timing: timing?.report(),
    receivedBytes,
    sentBytes,
    receivedFrames,
    heldResultBytes: heldBytes,
  }),
  holdResults: () => {
    holdingResults = true;
  },
  measure: (
    operation: "cold" | "reconnect" | "cacheLoss" | "refresh",
    address?: string,
    credential?: number[],
  ) => {
    if (activeMeasurement) throw Error("measurement already active");
    return new Promise((finish, fail) => {
      const timer = setTimeout(() => {
        activeMeasurement = undefined;
        fail(Error("presentation measurement deadline"));
      }, 25000);
      // Counters reset during cold construction, so its baseline is zero.
      activeMeasurement = {
        started: performance.now(),
        received: operation === "cold" ? 0 : receivedBytes,
        sent: operation === "cold" ? 0 : sentBytes,
        frames: operation === "cold" ? 0 : receivedFrames,
        finish,
        fail,
        timer,
      };
      painted = 0n;
      reconnectBusyRetries = 0;
      const run = async () => {
        if (operation === "cold") {
          await consumer.create(
            address!,
            credential!,
            false,
            Array(16).fill(3),
            true,
          );
          await connectWithCloseRetry();
        } else if (operation === "reconnect") {
          client!.disconnect();
          await connectWithCloseRetry();
        } else if (operation === "cacheLoss") client!.resourceCacheLost();
        else client!.refresh();
      };
      void run().catch((error) => {
        clearTimeout(timer);
        activeMeasurement = undefined;
        fail(error);
      });
    });
  },
};
declare global {
  interface Window {
    consumer: typeof consumer;
  }
}
window.consumer = consumer;
// Page restoration is caller-owned: preserve the actor ledger when cached,
// disconnect on departure, then explicitly connect after returning.
window.addEventListener("pagehide", () => {
  if (client) consumer.disconnect();
});
window.addEventListener("pageshow", (event) => {
  pageShows++;
  if (event.persisted) restoredFromCache++;
});
