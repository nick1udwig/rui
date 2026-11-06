import { DocumentRenderer } from "./document-renderer.mjs";
import { DocumentReceiver, DOCUMENT_PROFILE } from "./document-channel.mjs";
import { DocumentMediaView } from "./document-media.mjs";

export class DocumentView {
  constructor(container, api, dispatch, canControl, report) {
    this.container = container;
    this.api = api;
    this.report = report;
    this.receiver = new DocumentReceiver();
    this.resources = new Map();
    this.iframe = document.createElement("iframe");
    this.iframe.title = "Host web page";
    this.iframe.style.cssText = "width:100%;height:75dvh;border:0;background:white";
    this.media = new DocumentMediaView(api, report);
    this.renderer = new DocumentRenderer(this.iframe, dispatch, canControl, this.media);
    const sound = document.createElement("button");
    sound.textContent = "Enable page sound";
    sound.onclick = () => this.media.enableSound();
    this.container.append(sound);
    this.container.append(this.iframe);
    this.dispatch = dispatch;
    this.canControl = canControl;
    this.serial = 0;
  }
  start() {
    if (this.running) return;
    this.running = true;
    const serial = ++this.serial;
    const poll = async () => {
      if (!this.running || serial !== this.serial) return;
      try {
        if (document.visibilityState === "visible") {
          const packet = await this.api("document", { profile: DOCUMENT_PROFILE, revision: this.receiver.revision, generation: this.receiver.generation }, true);
          if (!this.running || serial !== this.serial) return;
          const snapshot = this.receiver.apply(packet);
          if (this.resourceGeneration !== snapshot.generation) { this.resources.clear(); this.resourceGeneration = snapshot.generation; }
          const hydrate = async snapshot => {
            for (const kind of ['resources', 'styleResources']) {
              const result = {};
              for (const [key, offer] of Object.entries(snapshot[kind] || {})) {
                if (typeof offer === 'string') { result[key] = offer; continue; }
                if (!/^[a-f0-9]{64}$/.test(key) || !Number.isSafeInteger(offer.bytes) || offer.bytes < 0 || offer.bytes > 8 * 1024 * 1024) throw Error('Resource offer rejected');
                if (!this.resources.has(key)) {
                  const { body } = await this.api('document-resource', { generation: packet.generation, key }, true);
                  if (typeof body !== 'string') throw Error('Invalid resource body');
                  const bytes = new TextEncoder().encode(body);
                  if (bytes.length !== offer.bytes) throw Error('Resource length mismatch');
                  const digest = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(n => n.toString(16).padStart(2,'0')).join('');
                  if (digest !== key) throw Error('Resource integrity mismatch');
                  if ([...this.resources.values()].reduce((n, v) => n + v.length, body.length) > 16 * 1024 * 1024) throw Error('Resource cache limit');
                  this.resources.set(key, body);
                }
                result[key] = this.resources.get(key);
              }
              snapshot[kind] = result;
            }
            const visit = async node => { if (node.document) await hydrate(node.document); for (const child of [...(node.children || []), ...(node.shadow || [])]) await visit(child); };
            await visit(snapshot.root);
          };
          await hydrate(snapshot);
          if (!this.running || serial !== this.serial) return;
          this.media.begin();
          await this.renderer.render(snapshot);
          this.media.finish();
          this.failures = 0;
          const width = Math.max(240, Math.min(2560, Math.round(this.iframe.clientWidth)));
          const height = Math.max(240, Math.min(2560, Math.round(this.iframe.clientHeight)));
          if (this.canControl() && (snapshot.width !== width || snapshot.height !== height)) this.dispatch({ generation: snapshot.generation, revision: snapshot.revision, action: "resize", width, height });
        }
      } catch (error) {
        this.media.dispose();
        this.iframe.hidden = true;
        this.report(error);
        this.receiver = new DocumentReceiver();
        this.renderer.generation = null;
        this.failures = (this.failures || 0) + 1;
      }
      if (!this.failures) this.iframe.hidden = false;
      if (this.running && serial === this.serial) this.timer = setTimeout(poll, this.failures ? Math.min(5000, this.failures * 1000) : 300);
    };
    void poll();
  }
  stop() {
    this.running = false;
    this.media.dispose();
    this.serial++;
    clearTimeout(this.timer);
    this.iframe.srcdoc = "";
    this.receiver = new DocumentReceiver();
    this.resources.clear();
    this.renderer.generation = null;
  }
}
