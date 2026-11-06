import { DocumentRenderer } from "./document-renderer.mjs";
import { DocumentReceiver, DOCUMENT_PROFILE } from "./document-channel.mjs";

export class DocumentView {
  constructor(container, api, dispatch, canControl, report) {
    this.container = container;
    this.api = api;
    this.report = report;
    this.receiver = new DocumentReceiver();
    this.iframe = document.createElement("iframe");
    this.iframe.title = "Host web page";
    this.iframe.style.cssText = "width:100%;height:75dvh;border:0;background:white";
    this.renderer = new DocumentRenderer(this.iframe, dispatch, canControl);
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
          await this.renderer.render(snapshot);
          const width = Math.max(240, Math.min(2560, Math.round(this.iframe.clientWidth)));
          const height = Math.max(240, Math.min(2560, Math.round(this.iframe.clientHeight)));
          if (this.canControl() && (snapshot.width !== width || snapshot.height !== height)) this.dispatch({ generation: snapshot.generation, revision: snapshot.revision, action: "resize", width, height });
        }
      } catch (error) {
        this.iframe.hidden = true;
        this.report(error);
        this.running = false;
        return;
      }
      this.iframe.hidden = false;
      if (this.running) this.timer = setTimeout(poll, 300);
    };
    void poll();
  }
  stop() {
    this.running = false;
    this.serial++;
    clearTimeout(this.timer);
    this.iframe.srcdoc = "";
    this.receiver = new DocumentReceiver();
    this.renderer.generation = null;
  }
}
