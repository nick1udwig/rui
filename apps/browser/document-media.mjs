import { randomBytes } from "node:crypto";

export class DocumentMedia {
  constructor(source) { this.source = source; this.sessions = new Map(); }
  async open(node, generation, transport = 'webrtc', types = [], quality) {
    const source = this.source;
    if (quality !== undefined && (!quality || !Number.isInteger(quality.level) || quality.level < 0 || quality.level > 2 || !Number.isInteger(quality.width) || quality.width < 2 || quality.width > 1920)) throw Error('Media quality rejected');
    if (!['webrtc', 'https'].includes(transport) || !Array.isArray(types) || types.length > 8 || types.some(type => typeof type !== 'string' || type.length > 128)) throw Error('Media transport rejected');
    if (generation !== source.generation || this.sessions.size >= 8) throw Error("Media scope rejected");
    const token = randomBytes(24).toString("hex");
    const session = { generation, expires: Date.now() + 10000 };
    this.sessions.set(token, session);
    try {
      const result = await source.frame.evaluate(async ({ key, node, token, transport, types, quality }) => {
        const state = globalThis[key];
        const element = state?.reverse.get(node);
        if (!element?.isConnected || !['canvas', 'video', 'audio'].includes(element.localName) || typeof element.captureStream !== 'function') throw Error('Media unavailable');
        const stream = element.localName === 'canvas' ? element.captureStream(30) : element.captureStream();
        if (!stream.getTracks().length) throw Error('Media has no tracks');
        state.media ||= new Map();
        const session = { stream, timer: null, visible: true };
        const tracks = stream.getTracks();
        const close = () => {
          if (session.closed) return;
          session.closed = true;
          clearTimeout(session.drawTimer);
          if (session.capturePlayer) { session.capturePlayer.pause(); session.capturePlayer.srcObject = null; }
          session.scaled?.getTracks().forEach(track => track.stop());
          for (const event of ['emptied', 'loadstart']) element.removeEventListener(event, close);
          stream.removeEventListener('removetrack', close);
          for (const track of new Set([...tracks, ...stream.getTracks()])) { track.removeEventListener('ended', close); track.stop(); }
          if (session.recorder?.state !== 'inactive') session.recorder?.stop();
          session.wake?.();
          session.peer?.close(); clearTimeout(session.timer); state.media.delete(token);
        };
        session.close = close;
        session.renew = () => { clearTimeout(session.timer); session.timer = setTimeout(close, 10000); };
        state.media.set(token, session);
        session.renew();
        for (const event of ['emptied', 'loadstart']) element.addEventListener(event, close);
        stream.addEventListener('removetrack', close);
        for (const track of tracks) track.addEventListener('ended', close);
        try {
          if (transport === 'https') {
            const video = !!stream.getVideoTracks().length, audio = !!stream.getAudioTracks().length;
            const allowed = ['video/webm;codecs=vp8,opus', 'video/webm;codecs=vp8', 'audio/webm;codecs=opus'];
            const mimeType = allowed.find(type => types.includes(type) && type.startsWith(video ? 'video/' : 'audio/') && type.includes('opus') === audio && MediaRecorder.isTypeSupported(type));
            if (!mimeType) throw Error('No supported HTTPS media codec');
            const preset = [{ width: 480, fps: 15, bitrate: 500000 }, { width: 960, fps: 30, bitrate: 1200000 }, { width: 1280, fps: 30, bitrate: 2000000 }][quality?.level ?? 2];
            let encoding = stream;
            if (video && quality) {
              const width = element.videoWidth || element.width, height = element.videoHeight || element.height;
              const scale = Math.min(1, Math.min(quality.width, preset.width) / width, 720 / height);
              const canvas = document.createElement('canvas');
              canvas.width = Math.max(2, Math.floor(width * scale / 2) * 2); canvas.height = Math.max(2, Math.floor(height * scale / 2) * 2);
              const context = canvas.getContext('2d', { alpha: false });
              let picture = element;
              if (element.localName === 'canvas') {
                picture = session.capturePlayer = document.createElement('video');
                picture.muted = true; picture.playsInline = true;
                picture.srcObject = new MediaStream(stream.getVideoTracks());
                void picture.play().catch(close);
              }
              const scaled = session.scaled = canvas.captureStream(0), track = scaled.getVideoTracks()[0];
              for (const audioTrack of stream.getAudioTracks()) scaled.addTrack(audioTrack);
              let lastTime;
              const draw = () => {
                if (session.closed) return;
                try {
                  const stamp = picture.getVideoPlaybackQuality?.().totalVideoFrames ?? picture.currentTime;
                  if (session.visible && picture.readyState >= 2 && stamp !== lastTime) {
                    context.drawImage(picture, 0, 0, canvas.width, canvas.height); track.requestFrame(); lastTime = stamp;
                  }
                  session.drawTimer = setTimeout(draw, 1000 / preset.fps);
                } catch { close(); }
              };
              session.startDrawing = draw; encoding = scaled;
              session.encoding = { width: canvas.width, height: canvas.height, fps: preset.fps, bitrate: preset.bitrate };
            }
            const recorder = session.recorder = new MediaRecorder(encoding, { mimeType, videoBitsPerSecond: preset.bitrate, audioBitsPerSecond: 96000 });
            session.chunks = []; session.bytes = 0; session.sequence = 0; session.ack = 0;
            let pending = Promise.resolve();
            recorder.ondataavailable = event => {
              if (session.closed || !event.data.size) return;
              session.bytes += event.data.size;
              if (session.bytes > 4 * 1024 * 1024) { close(); return; }
              pending = pending.then(async () => {
                const bytes = new Uint8Array(await event.data.arrayBuffer());
                if (session.closed) return;
                for (let start = 0; start < bytes.length; start += 256 * 1024) {
                  const part = bytes.subarray(start, start + 256 * 1024);
                  let binary = '';
                  for (let offset = 0; offset < part.length; offset += 16384) binary += String.fromCharCode(...part.subarray(offset, offset + 16384));
                  session.chunks.push({ sequence: ++session.sequence, data: btoa(binary), bytes: part.length });
                }
                session.wake?.();
              }).catch(close);
            };
            recorder.onerror = close;
            session.read = async (after, wait) => {
              if (!Number.isSafeInteger(after) || after < session.ack || after > session.sequence) throw Error('Media cursor rejected');
              while (session.chunks[0]?.sequence <= after) session.bytes -= session.chunks.shift().bytes;
              session.ack = after;
              if (wait && !session.chunks.length && !session.closed) await new Promise(resolve => {
                const timer = setTimeout(done, 500);
                function done() { clearTimeout(timer); session.wake = null; resolve(); }
                session.wake = done;
              });
              if (session.closed) throw Error("Media stream expired");
              const chunks = []; let bytes = 0;
              for (const chunk of session.chunks) {
                if (bytes + chunk.bytes > 512 * 1024 || chunks.length >= 32) break;
                chunks.push({ sequence: chunk.sequence, data: chunk.data }); bytes += chunk.bytes;
              }
              session.renew();
              return { chunks, ...(element.localName !== 'canvas' ? { playback: { paused: element.paused, time: element.currentTime, frames: element.getVideoPlaybackQuality?.().totalVideoFrames } } : {}) };
            };
            recorder.start(element.localName === "canvas" ? 100 : 250);
            session.startDrawing?.();
            return { transport, mimeType, encoding: session.encoding };
          }
          const peer = session.peer = new RTCPeerConnection({ iceServers: [] });
          for (const track of stream.getTracks()) peer.addTrack(track, stream);
          await peer.setLocalDescription(await peer.createOffer());
          await new Promise(resolve => {
            const timer = setTimeout(resolve, 2000);
            peer.addEventListener('icegatheringstatechange', () => { if (peer.iceGatheringState === 'complete') { clearTimeout(timer); resolve(); } });
            if (peer.iceGatheringState === 'complete') { clearTimeout(timer); resolve(); }
          });
          if (state.media.get(token) !== session) throw Error('Media source changed');
          return { transport, offer: { type: peer.localDescription.type, sdp: peer.localDescription.sdp } };
        } catch (error) { close(); throw error; }
      }, { key: source.key, node, token, transport, types, quality });
      if (source.generation !== generation || this.sessions.get(token) !== session) throw Error("Media document changed");
      session.expires = Date.now() + 10000;
      return { token, ...result };
    } catch (error) {
      await this.close(token);
      throw error;
    }
  }
  async read(token, after, wait = false) {
    const session = this.sessions.get(token);
    if (!session || session.generation !== this.source.generation || session.expires < Date.now()) throw Error('Media scope expired');
    const result = await this.source.frame.evaluate(({ key, token, after, wait }) => {
      const session = globalThis[key]?.media?.get(token);
      if (!session?.read) throw Error('Media stream expired');
      return session.read(after, wait);
    }, { key: this.source.key, token, after, wait });
    if (this.sessions.get(token) !== session || session.generation !== this.source.generation) throw Error('Media source changed');
    session.expires = Date.now() + 10000;
    return result;
  }
  async visibility(token, visible) {
    if (typeof visible !== 'boolean') throw Error('Media visibility rejected');
    const session = this.sessions.get(token);
    if (!session || session.generation !== this.source.generation) throw Error('Media scope expired');
    await this.source.frame.evaluate(({ key, token, visible }) => {
      const current = globalThis[key]?.media?.get(token);
      if (!current) throw Error('Media expired');
      current.visible = visible;
    }, { key: this.source.key, token, visible });
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
