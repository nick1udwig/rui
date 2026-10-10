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
export interface BrowserOptions {
  url: string;
  clientId: Uint8Array;
  credential: () => Promise<Uint8Array>;
  selection: string;
  canvas: HTMLCanvasElement;
  renderer?: Renderer;
  transport?: TransportFactory;
  allowInsecure?: boolean;
  scale?: number;
  onEvent?: (event: BrowserEvent) => void;
  timings?: Timings;
}
export type BrowserEvent =
  | SessionEvent
  | { type: "disconnected" }
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
/** The demo consumes this library. It knows no application labels, IDs, business
 * rules or GTK capture files. Its only authoritative graph is the Rust receiver. */
export class BrowserClient {
  private session: Session;
  private socket: BinaryTransport | undefined;
  private renderer: Renderer;
  private generation = 0;
  private frame = 0;
  private presentation = 0;
  private timer: ReturnType<typeof setInterval> | undefined;
  private connectReject: ((error: Error) => void) | undefined;
  private connectionDeadline: ReturnType<typeof setTimeout> | undefined;
  private active = false;
  private disposed = false;
  private visible = document.visibilityState === "visible";
  private canControl = false;
  private acquisition = false;
  private controlWanted = false;
  private snapshot: State | undefined;
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
    this.session = new Session(options.clientId, options.selection);
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
    await (initialization ??= init().catch((error) => {
      initialization = undefined;
      throw error;
    }));
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
    for (const listener of this.listeners) listener(event);
  }
  get state(): State | undefined {
    return this.snapshot;
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
    return this.session.pendingCommands;
  }
  get memoryUsed(): number {
    return this.session.memoryUsed;
  }
  get geometry(): Viewport {
    return { ...this.viewport, origin: { ...this.viewport.origin } };
  }
  setViewport(viewport: Viewport): void {
    this.viewportExplicit = true;
    this.viewport = { ...viewport, origin: { ...viewport.origin } };
    this.invalidate();
    if (this.active) this.schedule();
  }
  async connect(): Promise<void> {
    if (this.disposed || this.socket)
      throw new Error("client is disposed or already connected");
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
            if (event.type === "commandResult")
              this.options.timings?.result(
                event.sequence,
                event.revision,
                event.status,
              );
            if (event.type === "connected") {
              this.canControl = event.can_control;
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
            if (event.type === "reset") this.invalidate();
            if (event.type !== "none") this.emit(event);
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
    if (!this.visible || !this.active || !this.snapshot || this.frame) return;
    const generation = this.generation;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      const state = this.snapshot;
      if (!state || !this.visible || generation !== this.generation) return;
      try {
        const started = performance.now();
        this.renderer.render(state, this.viewport, (d) =>
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
            !this.options.canvas.isConnected ||
            !this.options.canvas.getClientRects().length
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
    const seq = this.session.activate(target);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  focus(target: Id): bigint {
    assertUint64(target);
    if (!this.hasControl) throw new Error("control unavailable");
    const seq = this.session.focus(target);
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
    assertUint64(capture.target);
    assertUint64(capture.revision);
    assertUint64(capture.textVersion);
    if (!(capture.token instanceof Uint8Array) || capture.token.length !== 32)
      throw new Error("invalid edit capture");
    if (!this.hasControl) throw new Error("control unavailable");
    const seq = this.session.replaceText(
      capture.target,
      capture.revision,
      capture.textVersion,
      content,
      capture.token,
    );
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  setSelection(target: Id, anchor: bigint, focus: bigint): bigint {
    assertUint64(target);
    assertUint64(anchor);
    assertUint64(focus);
    if (!this.hasControl) throw new Error("control unavailable");
    const seq = this.session.setSelection(target, anchor, focus);
    this.options.timings?.command(seq);
    this.flush();
    return seq;
  }
  retryPending(): void {
    this.session.retryPending();
    this.flush();
  }
  disconnect(): void {
    this.options.timings?.disconnect();
    this.resourceStarted = undefined;
    this.connectReject?.(new Error("connection cancelled"));
    this.connectReject = undefined;
    if (this.connectionDeadline) clearTimeout(this.connectionDeadline);
    this.connectionDeadline = undefined;
    ++this.generation;
    this.active = false;
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
    this.disconnect();
    this.disposed = true;
    document.removeEventListener("visibilitychange", this.visibility);
    this.session.close();
    this.session.free();
    this.renderer.dispose();
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
