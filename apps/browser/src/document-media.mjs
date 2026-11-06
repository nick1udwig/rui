export class DocumentMediaView {
  constructor(api, report) { this.api = api; this.report = report; this.streams = new Map(); }
  begin() { this.live = new Set(); }
  attach(node, element, generation, frames) {
    const key = `${generation}:${node.id}`;
    this.live.add(key);
    if (node.mediaReady === false) { this.close(key); return; }
    if (this.streams.has(key)) return;
    if (this.streams.size >= 8) { this.report(Error("Media region limit")); return; }
    const entry = { peer: new RTCPeerConnection({ iceServers: [] }), closed: false, element };
    this.streams.set(key, entry);
    const player = node.tag === "canvas" ? document.createElement("video") : element;
    entry.player = player;
    if (node.tag === "canvas") {
      player.style.cssText = "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none";
      player.setAttribute("aria-hidden", "true");
      document.body.append(player);
    }
    entry.deadline = setTimeout(() => {
      if (!entry.closed && player.readyState < 2) { this.close(key); this.report(Error("Media connection timed out")); }
    }, 10000);
    player.muted = !this.soundEnabled;
    player.autoplay = true;
    player.playsInline = true;
    entry.peer.ontrack = event => {
      if (entry.closed) return;
      if (player.srcObject !== event.streams[0]) player.srcObject = event.streams[0];
      void player.play().catch(error => { if (!entry.closed && error.name !== "AbortError") this.report(error); });
      if (node.tag === "canvas" && !entry.drawing) {
        entry.drawing = true;
        const draw = () => {
          if (entry.closed) return;
          if (player.readyState >= 2) element.getContext("2d").drawImage(player, 0, 0, element.width, element.height);
          entry.frame = player.requestVideoFrameCallback(draw);
        };
        draw();
      }
    };
    const open = async () => {
      const result = await this.api("media", { op: "open", generation, node: node.id, frames }, true);
      entry.token = result.token;
      if (entry.closed) { await this.api("media", { op: "close", token: entry.token }, true); return; }
      await entry.peer.setRemoteDescription(result.offer);
      await entry.peer.setLocalDescription(await entry.peer.createAnswer());
      await new Promise(resolve => {
        const timer = setTimeout(resolve, 2000);
        entry.peer.addEventListener("icegatheringstatechange", () => { if (entry.peer.iceGatheringState === "complete") { clearTimeout(timer); resolve(); } });
        if (entry.peer.iceGatheringState === "complete") { clearTimeout(timer); resolve(); }
      });
      await this.api("media", { op: "answer", token: entry.token, answer: entry.peer.localDescription.toJSON() }, true);
      const renew = async () => {
        if (entry.closed) return;
        try { await this.api("media", { op: "renew", token: entry.token }, true); }
        catch (error) { this.close(key); this.report(error); return; }
        entry.timer = setTimeout(renew, 3000);
      };
      if (!entry.closed) entry.timer = setTimeout(renew, 3000);
    };
    void open().catch(error => { this.close(key); this.report(error); });
  }
  finish() { for (const key of this.streams.keys()) if (!this.live.has(key)) this.close(key); }
  enableSound() { this.soundEnabled = true; for (const entry of this.streams.values()) { entry.player.muted = false; if (entry.player.srcObject) void entry.player.play().catch(this.report); } }
  close(key) {
    const entry = this.streams.get(key);
    if (!entry) return;
    entry.closed = true;
    clearTimeout(entry.timer);
    clearTimeout(entry.deadline);
    if (entry.frame) entry.player.cancelVideoFrameCallback(entry.frame);
    entry.peer.close();
    entry.player.srcObject = null;
    if (entry.player !== entry.element) entry.player.remove();
    this.streams.delete(key);
    if (entry.token) void this.api("media", { op: "close", token: entry.token }, true).catch(() => {});
  }
  dispose() { for (const key of this.streams.keys()) this.close(key); }
}
