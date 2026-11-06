import { randomBytes } from "node:crypto";

export class DocumentMedia {
  constructor(source) { this.source = source; this.sessions = new Map(); }
  async open(node, generation) {
    const source = this.source;
    if (generation !== source.generation || this.sessions.size >= 8) throw Error("Media scope rejected");
    const token = randomBytes(24).toString("hex");
    const session = { generation, expires: Date.now() + 10000 };
    this.sessions.set(token, session);
    try {
      const offer = await source.frame.evaluate(async ({ key, node, token }) => {
        const state = globalThis[key];
        const element = state?.reverse.get(node);
        if (!element?.isConnected || !['canvas', 'video', 'audio'].includes(element.localName) || typeof element.captureStream !== 'function') throw Error('Media unavailable');
        const stream = element.localName === 'canvas' ? element.captureStream(30) : element.captureStream();
        if (!stream.getTracks().length) throw Error('Media has no tracks');
        const peer = new RTCPeerConnection({ iceServers: [] });
        state.media ||= new Map();
        const session = { peer, stream, timer: null };
        const close = () => { peer.close(); for (const track of stream.getTracks()) track.stop(); clearTimeout(session.timer); state.media.delete(token); };
        session.close = close;
        session.renew = () => { clearTimeout(session.timer); session.timer = setTimeout(close, 10000); };
        state.media.set(token, session);
        session.renew();
        try {
          for (const track of stream.getTracks()) peer.addTrack(track, stream);
          await peer.setLocalDescription(await peer.createOffer());
          await new Promise(resolve => {
            const timer = setTimeout(resolve, 2000);
            peer.addEventListener('icegatheringstatechange', () => { if (peer.iceGatheringState === 'complete') { clearTimeout(timer); resolve(); } });
            if (peer.iceGatheringState === 'complete') { clearTimeout(timer); resolve(); }
          });
          return { type: peer.localDescription.type, sdp: peer.localDescription.sdp };
        } catch (error) { close(); throw error; }
      }, { key: source.key, node, token });
      if (source.generation !== generation || this.sessions.get(token) !== session) throw Error("Media document changed");
      session.expires = Date.now() + 10000;
      return { token, offer };
    } catch (error) {
      await this.close(token);
      throw error;
    }
  }
  async update(token, answer) {
    const session = this.sessions.get(token);
    if (!session || session.generation !== this.source.generation || session.expires < Date.now()) throw Error("Media scope expired");
    if (answer && (answer.type !== 'answer' || typeof answer.sdp !== 'string' || answer.sdp.length > 65536)) throw Error("Invalid media answer");
    await this.source.frame.evaluate(async ({ key, token, answer }) => {
      const session = globalThis[key]?.media?.get(token);
      if (!session) throw Error('Media expired');
      if (answer) await session.peer.setRemoteDescription(answer);
      session.renew();
    }, { key: this.source.key, token, answer });
    session.expires = Date.now() + 10000;
  }
  async close(token) {
    this.sessions.delete(token);
    await this.source.frame.evaluate(({ key, token }) => globalThis[key]?.media?.get(token)?.close(), { key: this.source.key, token }).catch(() => {});
  }
  async dispose() { await Promise.all([...this.sessions.keys()].map(token => this.close(token))); }
}
