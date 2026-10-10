import { CausalWait } from "./causal.js";
import type { Timings } from "./timing.js";
import init, { Session } from "../wasm/rui_client_wasm.js";
import { CanvasRenderer, type Renderer } from "./renderer.js";
import {
  assertUint64,
  semantic,
  text,
  type Digest,
  type Id,
  type SessionEvent,
  type State,
  type Viewport,
} from "./model.js";
export interface BinaryTransport {
  readonly bufferedAmount: number;
  /** Copy/admit the bytes synchronously; the caller clears its copy on return. */
  send(bytes: Uint8Array): void;
  close(): void;
}
export interface TransportCallbacks {
  opened(): void;
  message(bytes: ArrayBuffer): void;
  closed(): void;
  failed(): void;
}
export type TransportFactory = (
  url: string,
  callbacks: TransportCallbacks,
) => BinaryTransport;
/** Authentication and TLS belong to the transport. The default uses browser
 * certificate verification and the rui.v1 subprotocol, never URL credentials. */
export const webSocketTransport: TransportFactory = (url, callbacks) => {
  const ws = new WebSocket(url, "rui.v1");
  ws.binaryType = "arraybuffer";
  ws.onopen = () => {
    if (ws.protocol !== "rui.v1") {
      ws.close();
      callbacks.failed();
    } else callbacks.opened();
  };
  ws.onmessage = (event) => {
    if (event.data instanceof ArrayBuffer) callbacks.message(event.data);
    else {
      ws.close();
      callbacks.failed();
    }
  };
  ws.onclose = () => callbacks.closed();
  ws.onerror = () => callbacks.failed();
  return {
    get bufferedAmount() {
      return ws.bufferedAmount;
    },
    send: (bytes) => ws.send(bytes),
    close: () => ws.close(),
  };
};
export interface CommonOptions {
  url: string;
  clientId: Uint8Array;
  credential: () => Promise<Uint8Array>;
  selection: string;
  transport?: TransportFactory;
  allowInsecure?: boolean;
  scale?: number;
  onEvent?: (event: BrowserEvent) => void;
  timings?: Timings;
}
export type BrowserOptions = CommonOptions &
  (
    | { mode?: "mirror"; canvas: HTMLCanvasElement; renderer?: Renderer }
    | { mode: "semanticOnly"; canvas?: never; renderer?: never }
  );
export interface Capabilities {
  mode: "mirror" | "semanticOnly";
  profiles: string[];
  can_control: boolean;
  actions: string[];
}
export type ConnectionState =
  "idle" | "connecting" | "connected" | "disconnected" | "disposed";
export type BrowserEvent =
  | SessionEvent
  | { type: "disconnected" }
  | { type: "semanticReady"; revision: bigint }
  | { type: "presented"; revision: bigint; renderMs: number }
  | { type: "failure"; message: string };
export interface EditCapture {
  target: Id;
  revision: bigint;
  textVersion: bigint;
  content: string;
  token: Uint8Array;
}
let initialization: Promise<unknown> | undefined;
let initializationSource: string | undefined;
async function loadModule(asset: string | URL): Promise<WebAssembly.Module> {
  const response = await fetch(asset);
  if (!response.ok || !response.body)
    throw new Error(`WASM asset request failed (${response.status})`);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.length;
      if (length > 8 * 1024 * 1024 || chunks.length === 1024)
        throw new Error("WASM initialization asset limit");
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel();
    throw error;
  } finally {
    reader.releaseLock();
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  const module = await WebAssembly.compile(bytes);
  const exports = WebAssembly.Module.exports(module);
  for (const name of [
    "session_new",
    "session_receive",
    "session_close",
    "session_pendingCommands",
    "session_setRequestNamespace",
    "session_memoryPeak",
    "__wbindgen_start",
  ])
    if (!exports.some((item) => item.name === name && item.kind === "function"))
      throw new Error("incompatible WASM session ABI");
  if (
    !exports.some((item) => item.name === "memory" && item.kind === "memory") ||
    WebAssembly.Module.imports(module).some(
      (item) => item.module === "wasi_snapshot_preview1",
    )
  )
    throw new Error("incompatible browser WASM module");
  if (!response.headers.get("content-type")?.startsWith("application/wasm"))
    console.warn(
      "WASM asset should be served as application/wasm; using buffered initialization",
    );
  return module;
}
/** Initialize once per module instance. Bundlers resolve the bundled WASM by
 * default; an explicit URL supports separately hosted immutable assets. Failed
 * initialization can be retried. A loaded ABI cannot be replaced in place. */
export async function initialize(wasmUrl?: string | URL): Promise<void> {
  const source =
    wasmUrl === undefined
      ? undefined
      : new URL(wasmUrl, globalThis.location?.href).href;
  if (initialization && source && source !== initializationSource)
    throw new Error("WASM already initialized from a different asset");
  if (!initialization) {
    initializationSource = source ?? "bundled";
    initialization = loadModule(
      source ?? new URL("../wasm/rui_client_wasm_bg.wasm", import.meta.url),
    )
      .then((module) => init({ module_or_path: module }))
      .catch((error) => {
        initialization = undefined;
        initializationSource = undefined;
        throw error;
      });
  }
  await initialization;
}
/** The demo consumes this library. It knows no application labels, IDs, business
 * rules or GTK capture files. Its only authoritative graph is the Rust receiver. */
export class BrowserClient {
  private session: Session;
  private socket: BinaryTransport | undefined;
  private renderer: Renderer | undefined;
  private generation = 0;
  private frame = 0;
  private presentation = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private connectReject: ((error: Error) => void) | undefined;
  private connectionDeadline: ReturnType<typeof setTimeout> | undefined;
  private active = false;
  private connecting = false;
  private connection: ConnectionState = "idle";
  private disposed = false;
  private visible = document.visibilityState === "visible";
  private canControl = false;
  private acquisition = false;
  private controlWanted = false;
  private snapshot: State | undefined;
  private causal = new CausalWait();
  private viewport: Viewport;
  private viewportExplicit = false;
  private displayedRevision: bigint | undefined;
  private resourceStarted: number | undefined;
  private listeners = new Set<(event: BrowserEvent) => void>();
  private visibility = () => {
    const visible = document.visibilityState === "visible";
    if (visible === this.visible) return;
    this.visible = visible;
    this.invalidate();
    if (!this.visible) {
      this.controlWanted = false;
      this.acquisition = false;
      try {
        this.session.release();
        this.flush();
      } catch {}
      this.emit({ type: "control", granted: false });
    } else if (this.active && this.session.revision > 0n) this.schedule();
  };
  private constructor(private options: BrowserOptions) {
    this.session = new Session(
      options.clientId,
      options.selection,
      options.mode === "semanticOnly",
    );
    this.session.setRequestNamespace(
      crypto.getRandomValues(new Uint8Array(16)),
    );
    if (options.mode !== "semanticOnly")
      this.renderer =
        options.renderer ??
        new CanvasRenderer(options.canvas, undefined, options.timings);
    this.viewport = {
      width: 1,
      height: 1,
      origin: { x: 0, y: 0 },
      scale: options.scale ?? 0.75,
    };
    if (options.onEvent) this.listeners.add(options.onEvent);
    document.addEventListener("visibilitychange", this.visibility);
  }
  static async create(options: BrowserOptions): Promise<BrowserClient> {
    await initialize();
    const url = new URL(options.url, location.href);
    if (url.username || url.password || url.search)
      throw new Error("use a separate credential provider");
    if (
      url.protocol !== "wss:" &&
      !(url.protocol === "ws:" && options.allowInsecure)
    )
      throw new Error("a trusted wss URL is required");
    return new BrowserClient({
      ...options,
      url: url.href,
      clientId: options.clientId.slice(),
    });
  }
  subscribe(listener: (event: BrowserEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  private emit(event: BrowserEvent): void {
    const generation = this.generation;
    for (const listener of [...this.listeners]) {
      if (!this.listeners.has(listener)) continue;
      listener(event);
      if (this.disposed || generation !== this.generation) break;
    }
  }
  get state(): State | undefined {
    return this.snapshot && structuredClone(this.snapshot);
  }
  get capabilities(): Capabilities {
    if (this.disposed)
      return {
        mode: this.options.mode ?? "mirror",
        profiles: [],
        can_control: false,
        actions: [],
      };
    return this.session.capabilities() as Capabilities;
  }
  get connectionState(): ConnectionState {
    return this.connection;
  }
  get hasControl(): boolean {
    return (
      this.active &&
      this.visible &&
      document.visibilityState === "visible" &&
      this.displayedRevision === this.session.revision &&
      this.session.hasControl
    );
  }
  get pendingCommands(): number {
    if (this.disposed) return 0;
    return Math.max(this.session.pendingCommands, Number(this.causal.pending));
  }
  get memoryUsed(): number {
    if (this.disposed) return 0;
    return this.session.memoryUsed;
  }
  get resourcesReady(): boolean {
    if (this.disposed) return false;
    return this.session.resourcesReady;
  }
  /** Conservative shared Rust ownership high-water, separate from JS/socket/heap capacity. */
  get memoryPeak(): number {
    return this.disposed ? 0 : this.session.memoryPeak;
  }
  get geometry(): Viewport {
    return { ...this.viewport, origin: { ...this.viewport.origin } };
  }
  /** Owned verified bytes for a custom renderer; unauthorized/missing IDs reject. */
  resourceBytes(digest: Uint8Array): Uint8Array {
    if (!(digest instanceof Uint8Array) || digest.length !== 32)
      throw new RangeError("resource digest");
    return this.session.resourceBytes(digest);
  }
  /** Repaint the current committed graph without requesting source layout. */
  refresh(): void {
    if (this.options.mode === "semanticOnly")
      throw new Error("semantic consumers accept state explicitly");
    this.invalidate();
    this.schedule();
  }
  /** Trusted local cache-loss policy. It preserves records and exact commands. */
  resourceCacheLost(): void {
    const event = this.session.resourceCacheLost() as SessionEvent;
    if (event.type === "resourcesNeeded" || event.type === "applied") {
      this.invalidate();
      this.renderer?.reset?.();
      if (event.type === "applied") {
        this.snapshot = this.session.state() as State;
        this.schedule();
      }
      this.emit(event);
    }
    this.flush();
  }
  /** Evict one retired unpinned body; live and pending closure pins remain. */
  evictUnpinnedResource(): Uint8Array | undefined {
    const digest = this.session.evictUnpinnedResource();
    this.flush();
    return digest;
  }
  setViewport(viewport: Viewport): void {
    if (this.options.mode === "semanticOnly")
      throw new Error("semantic layout is local to the consumer");
    this.viewportExplicit = true;
    this.viewport = { ...viewport, origin: { ...viewport.origin } };
    this.invalidate();
    if (this.active) this.schedule();
  }
  async connect(): Promise<void> {
    if (this.disposed || this.socket || this.connecting)
      throw new Error("client is disposed or already connected");
    this.connecting = true;
    this.connection = "connecting";
    try {
      await this.connectInner();
    } finally {
      this.connecting = false;
      if (this.connection === "connecting") this.connection = "disconnected";
    }
  }
  private async connectInner(): Promise<void> {
    const generation = ++this.generation;
    const supplied = await this.options.credential();
    const credential = supplied.slice();
    if (generation !== this.generation || this.disposed) {
      credential.fill(0);
      throw new Error("connection cancelled");
    }
    try {
      this.session.start(credential);
    } finally {
      credential.fill(0);
    }
    let resolve!: () => void, reject!: (error: Error) => void;
    const ready = new Promise<void>((a, b) => {
      resolve = a;
      reject = b;
    });
    this.connectReject = reject;
    const fail = (message: string) => {
      if (generation !== this.generation) return;
      this.emit({ type: "failure", message });
      reject(new Error(message));
      this.disconnect();
    };
    const timeout = setTimeout(() => fail("connection timed out"), 10000);
    this.connectionDeadline = timeout;
    const factory = this.options.transport ?? webSocketTransport;
    try {
      this.socket = factory(this.options.url, {
        opened: () => {
          if (generation === this.generation) {
            this.active = true;
            this.flush();
          }
        },
        message: (bytes) => {
          if (generation !== this.generation) return;
          this.visibility();
          if (bytes.byteLength > this.session.ceiling) {
            fail("message limit exceeded");
            return;
          }
          try {
            const received = this.options.timings ? performance.now() : 0;
            const event = this.session.receive(
              new Uint8Array(bytes),
            ) as SessionEvent;
            this.options.timings?.record(
              "receipt",
              this.session.revision,
              performance.now() - received,
              bytes.byteLength,
            );
            if (
              this.options.timings &&
              event.type === "resourcesNeeded" &&
              this.resourceStarted === undefined
            )
              this.resourceStarted = performance.now();
            if (event.type === "control") {
              this.acquisition = false;
              if (!event.granted) this.controlWanted = false;
            }
            if (event.type === "error") {
              this.acquisition = false;
              this.controlWanted = false;
              this.emit({ type: "control", granted: false });
            }
            if (generation !== this.generation || this.disposed) return;
            if (event.type === "commandResult") {
              this.causal.result(event.sequence, event.status, event.revision);
              this.options.timings?.result(
                event.sequence,
                event.revision,
                event.status,
              );
            }
            if (event.type === "connected") {
              this.canControl = event.can_control;
              this.connection = "connected";
              clearTimeout(timeout);
              resolve();
            }
            if (
              event.type === "error" &&
              event.code === "PRODUCER_CONNECTION_BUSY"
            ) {
              fail(
                "host is still closing the previous connection; reconnect shortly",
              );
              return;
            }
            if (event.type === "error" && !this.snapshot) {
              fail(event.code);
              return;
            }
            if (event.type === "error") {
              this.controlWanted = false;
              this.acquisition = false;
              this.emit({ type: "control", granted: false });
            }
            if (event.type === "control") {
              this.acquisition = false;
              if (!event.granted) this.controlWanted = false;
            }
            if (event.type === "applied" || event.type === "resumed") {
              if (this.resourceStarted !== undefined) {
                this.options.timings?.record(
                  "resourceWait",
                  this.session.revision,
                  performance.now() - this.resourceStarted,
                );
                this.resourceStarted = undefined;
              }
              const exported = this.options.timings ? performance.now() : 0;
              this.snapshot = this.session.state() as State;
              this.options.timings?.record(
                "wasmExport",
                this.session.revision,
                performance.now() - exported,
                this.snapshot.records.size,
              );
              this.invalidate();
              const session = [...this.snapshot.records.values()].find(
                (r) => "Session" in r,
              );
              if (!this.viewportExplicit && session && "Session" in session) {
                const c = session.Session.canvas;
                this.viewport = {
                  width: Math.ceil(c.width * this.viewport.scale),
                  height: Math.ceil(c.height * this.viewport.scale),
                  origin: { x: c.x, y: c.y },
                  scale: this.viewport.scale,
                };
              }
              this.schedule();
            }
            if (event.type === "reset") {
              this.causal.clear();
              this.renderer?.reset?.();
              this.snapshot = undefined;
              this.invalidate();
            }
            if (event.type !== "none") this.emit(event);
            if (generation !== this.generation || this.disposed) return;
            this.flush();
          } catch (error) {
            fail(error instanceof Error ? error.message : String(error));
          }
        },
        closed: () => {
          if (generation === this.generation) {
            clearTimeout(timeout);
            reject(new Error("host closed the connection"));
            this.disconnect();
          }
        },
        failed: () => fail("WebSocket connection failed"),
      });
    } catch (error) {
      clearTimeout(timeout);
      this.session.disconnect();
      throw error;
    }
    this.timer = setInterval(() => {
      if (!this.active) return;
      try {
        // Also sample visibility during I/O/ticks: some browser transitions
        // update the property before delivering the visibilitychange event.
        this.visibility();
        this.session.tick(BigInt(Math.floor(performance.now())));
        this.flush();
      } catch (error) {
        fail(String(error));
      }
    }, 100);
    try {
      await ready;
    } finally {
      clearTimeout(timeout);
      this.connectReject = undefined;
      this.connectionDeadline = undefined;
    }
  }
  private flush(): void {
    if (!this.socket || !this.active) return;
    while (this.socket.bufferedAmount < this.session.ceiling * 2) {
      const frame = this.session.nextOutbound();
      if (!frame) break;
      try {
        this.socket.send(frame);
      } finally {
        frame.fill(0);
      }
    }
    if (this.socket.bufferedAmount > this.session.ceiling * 3)
      throw new Error("outbound transport limit");
  }
  private invalidate(): void {
    this.displayedRevision = undefined;
    if (this.frame) cancelAnimationFrame(this.frame);
    if (this.presentation) cancelAnimationFrame(this.presentation);
    this.frame = 0;
    this.presentation = 0;
  }
  private schedule(): void {
    if (this.options.mode === "semanticOnly") return;
    if (
      !this.visible ||
      !this.active ||
      !this.snapshot ||
      this.frame ||
      !this.resourcesReady
    )
      return;
    const generation = this.generation;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      const state = this.snapshot;
      if (!state || !this.visible || generation !== this.generation) return;
      try {
        const started = performance.now();
        this.renderer!.render(state, this.viewport, (d) =>
          this.session.resourceBytes(new Uint8Array(d)),
        );
        const renderMs = performance.now() - started;
        // A second visible animation frame is an opportunity for the previous
        // canvas submission to have painted. It is not physical scanout evidence.
        this.presentation = requestAnimationFrame(() => {
          this.presentation = 0;
          this.visibility();
          if (
            !this.visible ||
            !this.active ||
            generation !== this.generation ||
            state.revision !== this.session.revision ||
            !this.options.canvas?.isConnected ||
            !this.options.canvas?.getClientRects().length
          )
            return;
          try {
            this.options.timings?.record(
              "presentationOpportunity",
              state.revision,
              performance.now() - started - renderMs,
            );
            this.session.presented(state.revision);
            this.displayedRevision = state.revision;
            this.causal.present(state.revision);
            this.options.timings?.present(state.revision);
            this.flush();
            this.emit({
              type: "presented",
              revision: state.revision,
              renderMs,
            });
            if (this.controlWanted) this.requestControl();
          } catch (error) {
            this.emit({ type: "failure", message: String(error) });
          }
        });
      } catch (error) {
        this.emit({ type: "failure", message: String(error) });
        this.disconnect();
      }
    });
  }
  /** Call after a semantic consumer has accepted the current committed model. */
  semanticReady(revision: bigint): void {
    assertUint64(revision);
    this.visibility();
    if (
      this.options.mode !== "semanticOnly" ||
      !this.active ||
      !this.visible ||
      !this.snapshot ||
      this.snapshot.revision !== revision
    )
      throw new Error("semantic readiness unavailable");
    this.session.semanticReady(revision);
    this.displayedRevision = revision;
    this.causal.present(revision);
    this.flush();
    const generation = this.generation;
    this.emit({ type: "semanticReady", revision });
    if (generation === this.generation && !this.disposed && this.controlWanted)
      this.requestControl();
  }
  requestControl(): void {
    this.visibility();
    this.controlWanted = true;
    if (
      !this.canControl ||
      !this.visible ||
      this.displayedRevision !== this.session.revision ||
      this.hasControl ||
      this.acquisition
    )
      return;
    try {
      this.session.acquire(BigInt(Math.floor(performance.now())));
      this.acquisition = true;
      this.flush();
    } catch {
      /* Defer until the current revision has a presentation opportunity. */
    }
  }
  releaseControl(): void {
    this.controlWanted = false;
    this.acquisition = false;
    this.session.release();
    this.flush();
    this.emit({ type: "control", granted: false });
  }
  targetAt(x: number, y: number): Id {
    if (this.options.mode === "semanticOnly")
      throw new Error("semantic targets use identities");
    if (this.displayedRevision !== this.session.revision)
      throw new Error("wait for presentation");
    const v = this.viewport;
    return this.session.hitTest(
      x,
      y,
      v.width,
      v.height,
      v.origin.x,
      v.origin.y,
      v.scale,
    );
  }
  activate(target: Id): bigint {
    assertUint64(target);
    if (!this.hasControl) throw new Error("control unavailable");
    if (this.pendingCommands) throw new Error("wait for the preceding command");
    const seq = this.session.activate(target);
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  focus(target: Id): bigint {
    assertUint64(target);
    if (!this.hasControl) throw new Error("control unavailable");
    if (this.pendingCommands) throw new Error("wait for the preceding command");
    const seq = this.session.focus(target);
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  setChecked(target: Id, checked: boolean): bigint {
    assertUint64(target);
    if (typeof checked !== "boolean")
      throw new TypeError("expected a boolean value");
    if (!this.hasControl || this.pendingCommands)
      throw new Error("wait for current control and readiness");
    const seq = this.session.setChecked(target, checked);
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  activateItem(collection: Id, item: Id, modelVersion: bigint): bigint {
    assertUint64(collection);
    assertUint64(item);
    assertUint64(modelVersion);
    if (!this.hasControl || this.pendingCommands)
      throw new Error("wait for current control and readiness");
    const seq = this.session.activateItem(collection, item, modelVersion);
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  captureEdit(target: Id): EditCapture {
    assertUint64(target);
    const state = this.snapshot,
      model = state && text(state, target);
    const session =
      state && [...state.records.values()].find((r) => "Session" in r);
    if (
      !state ||
      this.displayedRevision !== state.revision ||
      !this.hasControl ||
      !model?.editable ||
      model.content === null ||
      !session ||
      !("Session" in session) ||
      session.Session.focus?.id !== target ||
      this.pendingCommands
    )
      throw new Error("wait for host focus and control");
    return {
      target,
      revision: state.revision,
      textVersion: model.text_version,
      content: model.content,
      token: this.session.editToken(target),
    };
  }
  commitEdit(capture: EditCapture, content: string): bigint {
    if (
      typeof content !== "string" ||
      content.length > 16 << 10 ||
      content.includes("\0") ||
      new TextEncoder().encode(content).length > 16 << 10
    )
      throw new RangeError("text commit limit");
    assertUint64(capture.target);
    assertUint64(capture.revision);
    assertUint64(capture.textVersion);
    if (!(capture.token instanceof Uint8Array) || capture.token.length !== 32)
      throw new Error("invalid edit capture");
    if (!this.hasControl) throw new Error("control unavailable");
    if (this.pendingCommands) throw new Error("wait for the preceding command");
    const seq = this.session.replaceText(
      capture.target,
      capture.revision,
      capture.textVersion,
      content,
      capture.token,
    );
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  setSelection(target: Id, anchor: bigint, focus: bigint): bigint {
    assertUint64(target);
    assertUint64(anchor);
    assertUint64(focus);
    if (!this.hasControl) throw new Error("control unavailable");
    if (this.pendingCommands) throw new Error("wait for the preceding command");
    const seq = this.session.setSelection(target, anchor, focus);
    this.causal.begin(seq);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  retryPending(): void {
    this.session.retryPending();
    this.flush();
  }
  disconnect(): void {
    if (!this.disposed) this.connection = "disconnected";
    this.options.timings?.disconnect();
    this.resourceStarted = undefined;
    this.connectReject?.(new Error("connection cancelled"));
    this.connectReject = undefined;
    if (this.connectionDeadline) clearTimeout(this.connectionDeadline);
    this.connectionDeadline = undefined;
    ++this.generation;
    this.active = false;
    this.causal.clear();
    this.renderer?.reset?.();
    this.snapshot = undefined;
    this.canControl = false;
    this.acquisition = false;
    this.controlWanted = false;
    const socket = this.socket;
    this.socket = undefined;
    socket?.close();
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
    this.invalidate();
    this.session.disconnect();
    this.emit({ type: "disconnected" });
  }
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.connection = "disposed";
    this.disconnect();
    document.removeEventListener("visibilitychange", this.visibility);
    this.session.close();
    this.session.free();
    this.renderer?.dispose();
    this.snapshot = undefined;
    this.listeners.clear();
  }
}
/** Maps pixels to published semantic controls; label/fixture knowledge stays out. */
export function bindPointer(
  client: BrowserClient,
  canvas: HTMLCanvasElement,
  onText: (target: Id) => void,
  onError: (message: string) => void,
): () => void {
  const click = (event: PointerEvent) => {
    if (!client.hasControl) return;
    try {
      const r = canvas.getBoundingClientRect(),
        g = client.geometry;
      const target = client.targetAt(
          ((event.clientX - r.left) * g.width) / r.width,
          ((event.clientY - r.top) * g.height) / r.height,
        ),
        n = client.state && semantic(client.state, target);
      if (n?.text && n.actions.some((a) => a.id === "Focus")) {
        client.focus(target);
        onText(target);
      } else if (n?.actions.some((a) => a.id === "Activate"))
        client.activate(target);
    } catch (error) {
      onError(String(error));
    }
  };
  canvas.addEventListener("pointerup", click);
  return () => canvas.removeEventListener("pointerup", click);
}
