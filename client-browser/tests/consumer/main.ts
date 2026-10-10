import {
  BrowserClient,
  CanvasRenderer,
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
    capabilities: client?.capabilities,
    renders,
    sends,
    releases,
    text: editable()?.text ? text(state!, editable()!.id)?.content : null,
    failures: events.filter((e) => e.type === "failure"),
    results: events.filter((e) => e.type === "commandResult"),
    events: events.slice(-12),
    state,
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
  ) => {
    client?.dispose();
    events = [];
    painted = 0n;
    renders = sends = releases = 0;
    const options: BrowserOptions = {
      url,
      credential: async () => new Uint8Array(credential),
      clientId: new Uint8Array(clientId),
      selection: "fixture",
      canvas,
      allowInsecure: url.startsWith("ws:"),
      scale: 1,
      onEvent: (event) => {
        events.push(event);
        if (event.type === "presented") painted = event.revision;
      },
    };
    if (replacement) {
      const delegate = new CanvasRenderer(canvas);
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
        const delegate = webSocketTransport(address, callbacks);
        return {
          get bufferedAmount() {
            return delegate.bufferedAmount;
          },
          send(bytes) {
            sends++;
            const owned = bytes.slice();
            try {
              delegate.send(owned);
            } finally {
              owned.fill(0);
            }
          },
          close() {
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
  connect: () => client!.connect(),
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
};
declare global {
  interface Window {
    consumer: typeof consumer;
  }
}
window.consumer = consumer;
