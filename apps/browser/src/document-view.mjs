import { DocumentRenderer } from "./document-renderer.mjs";
import { DocumentReceiver, DOCUMENT_PROFILE } from "./document-channel.mjs";
import { concurrent, resourceBatches } from "./document-work.mjs";
import { DocumentMediaView } from "./document-media.mjs";
import { documentRequest } from './document-request.mjs';

export class DocumentView {
  constructor(container, api, dispatch, canControl, report, onRendered = () => {}) {
    this.container = container;
    this.api = api;
    this.report = report;
    this.onRendered = onRendered;
    this.receiver = new DocumentReceiver();
    this.resources = new Map();
    this.resourceSizes = new Map();
    this.resourceBytes = 0;
    this.iframe = document.createElement("iframe");
    this.iframe.title = "Host web page";
    this.iframe.style.cssText = "width:100%;height:75dvh;border:0;background:white";
    this.status = document.createElement("p");
    this.status.setAttribute("role", "status");
    this.status.hidden = true;
    this.mediaStatus = document.createElement("p");
    this.mediaStatus.setAttribute("role", "status");
    this.mediaStatus.hidden = true;
    this.media = new DocumentMediaView(api, error => {
      this.mediaStatus.textContent = `Video/audio connection: ${String(error)}`;
      this.mediaStatus.hidden = false;
    });
    this.renderer = new DocumentRenderer(this.iframe, dispatch, canControl, this.media);
    const sound = document.createElement("button");
    sound.textContent = "Enable page sound";
    sound.onclick = () => this.media.enableSound();
    this.container.append(sound, this.status, this.mediaStatus);
    this.container.append(this.iframe);
    this.dispatch = dispatch;
    this.canControl = canControl;
    this.serial = 0;
    this.requestTimeout = 15000;
  }
  get generation() { return this.renderer.generation; }
  start() {
    if (this.running) return;
    this.running = true;
    const serial = ++this.serial;
    const controller = this.controller = new AbortController();
    const request = (path, data) => documentRequest(this.api, path, data, controller.signal, this.requestTimeout);
    const poll = async () => {
      if (!this.running || serial !== this.serial) return;
      let rendering = false;
      try {
        if (document.visibilityState === "visible") {
          const packet = await request("document", { profile: DOCUMENT_PROFILE, revision: this.receiver.revision, generation: this.receiver.generation });
          if (!this.running || serial !== this.serial) return;
          const snapshot = this.receiver.apply(packet);
          const required = new Map();
          const documents = [];
          const collect = snapshot => {
            documents.push(snapshot);
            for (const kind of ['resources', 'styleResources']) for (const [key, offer] of Object.entries(snapshot[kind] || {})) {
              if (typeof offer === 'string') continue;
              if (!/^[a-f0-9]{64}$/.test(key) || !Number.isSafeInteger(offer.bytes) || offer.bytes < 0 || offer.bytes > 8 * 1024 * 1024) throw Error('Resource offer rejected');
              if (required.has(key) && required.get(key).bytes !== offer.bytes) throw Error('Resource offer conflict');
              required.set(key, offer);
            }
            const visit = node => { if (node.document) collect(node.document); for (const child of [...(node.children || []), ...(node.shadow || [])]) visit(child); };
            visit(snapshot.root);
          };
          collect(snapshot);
          for (const [key, body] of this.resources) if (!required.has(key)) { this.resources.delete(key); this.resourceBytes -= this.resourceSizes.get(key); this.resourceSizes.delete(key); }
          if ([...required.values()].reduce((bytes, offer) => bytes + offer.bytes, 0) > 16 * 1024 * 1024) throw Error('Resource cache limit');
          const missing = [...required].filter(([key, offer]) => {
            if (!this.resources.has(key)) return true;
            if (this.resourceSizes.get(key) !== offer.bytes) throw Error('Resource length mismatch');
            return false;
          });
          await concurrent(resourceBatches(missing), 4, async batch => {
            const { bodies } = await request('document-resource', { generation: packet.generation, keys: batch.map(([key]) => key) });
            if (!this.running || serial !== this.serial) return;
            if (!bodies || typeof bodies !== 'object' || Object.keys(bodies).length !== batch.length) throw Error('Invalid resource batch');
            for (const [key, offer] of batch) {
              const body = bodies[key];
              if (typeof body !== 'string') throw Error('Invalid resource body');
              const bytes = new TextEncoder().encode(body);
              if (bytes.length !== offer.bytes) throw Error('Resource length mismatch');
              const digest = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(n => n.toString(16).padStart(2,'0')).join('');
              if (!this.running || serial !== this.serial) return;
              if (digest !== key) throw Error('Resource integrity mismatch');
              if (this.resourceBytes + bytes.length > 16 * 1024 * 1024) throw Error('Resource cache limit');
              this.resources.set(key, body);
              this.resourceSizes.set(key, bytes.length);
              this.resourceBytes += bytes.length;
            }
          });
          for (const document of documents) for (const kind of ['resources', 'styleResources']) {
            document[kind] = Object.fromEntries(Object.entries(document[kind] || {}).map(([key, offer]) => [key, typeof offer === 'string' ? offer : this.resources.get(key)]));
          }
          if (!this.running || serial !== this.serial) return;
          const previousGeneration = this.displayedGeneration;
          rendering = true;
          this.media.begin();
          await this.renderer.render(snapshot, controller.signal);
          if (!this.running || serial !== this.serial) return;
          this.displayedGeneration = snapshot.generation;
          this.media.finish();
          rendering = false;
          this.failures = 0;
          this.iframe.hidden = false;
          this.iframe.inert = false;
          this.status.hidden = true;
          this.container.removeAttribute("aria-busy");
          if (!this.media.failures.size && [...this.media.streams.values()].every(entry => entry.peer.connectionState === "connected" && !entry.blocked)) this.mediaStatus.hidden = true;
          if (previousGeneration && previousGeneration !== snapshot.generation && this.canControl()) {
            this.container.style.scrollMarginTop = `${(document.querySelector("header")?.offsetHeight || 0) + 8}px`;
            this.container.scrollIntoView({ block: "start" });
          }
          const width = Math.max(240, Math.min(2560, Math.round(this.iframe.clientWidth)));
          const height = Math.max(240, Math.min(2560, Math.round(this.iframe.clientHeight)));
          const viewport = `${snapshot.generation}:${width}:${height}`;
          if (!this.canControl() || (snapshot.width === width && snapshot.height === height)) this.requestedViewport = null;
          else if (this.requestedViewport !== viewport && (snapshot.width !== width || snapshot.height !== height)) {
            this.requestedViewport = viewport;
            this.dispatch({ generation: snapshot.generation, revision: snapshot.revision, action: "resize", width, height });
          }
          this.onRendered();
        }
      } catch (error) {
        if (!this.running || serial !== this.serial) return;
        if (rendering) { this.media.dispose(); this.renderer.generation = null; }
        this.iframe.hidden = !this.displayedGeneration;
        this.iframe.inert = true;
        this.container.setAttribute("aria-busy", "true");
        this.status.textContent = "Updating the host page… The previous view stays visible until the update is ready.";
        this.status.hidden = false;
        this.report(error);
        this.receiver = new DocumentReceiver();
        this.failures = (this.failures || 0) + 1;
      }
      if (!this.failures) this.iframe.hidden = false;
      if (this.running && serial === this.serial) this.timer = setTimeout(poll, this.failures ? Math.min(5000, this.failures * 1000) : 300);
    };
    void poll();
  }
  stop() {
    this.running = false;
    this.controller?.abort();
    this.renderer.cancel();
    this.displayedGeneration = null;
    this.status.hidden = true;
    this.mediaStatus.hidden = true;
    this.container.removeAttribute("aria-busy");
    this.requestedViewport = null;
    this.media.dispose();
    this.serial++;
    clearTimeout(this.timer);
    this.iframe.srcdoc = "";
    this.receiver = new DocumentReceiver();
    this.resources.clear();
    this.resourceSizes.clear();
    this.resourceBytes = 0;
    this.renderer.generation = null;
  }
}
