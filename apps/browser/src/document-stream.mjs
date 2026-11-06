const TYPES = ['video/webm;codecs=vp8,opus', 'video/webm;codecs=vp8', 'audio/webm;codecs=opus'];

export function streamTypes() {
  return TYPES.filter(type => globalThis.MediaSource?.isTypeSupported(type));
}

function completion(target, event, signal, start) {
  return new Promise((resolve, reject) => {
    const done = error => {
      clearTimeout(timer);
      target.removeEventListener(event, success);
      target.removeEventListener('error', failure);
      signal.removeEventListener('abort', aborted);
      if (error) reject(error); else resolve();
    };
    const success = () => done(), failure = () => done(Error('HTTPS media decode failed'));
    const aborted = () => done(signal.reason);
    const timer = setTimeout(() => done(Error('HTTPS media buffer timed out')), 8000);
    target.addEventListener(event, success, { once: true });
    target.addEventListener('error', failure, { once: true });
    signal.addEventListener('abort', aborted, { once: true });
    try { signal.throwIfAborted(); start?.(); } catch (error) { done(error); }
  });
}

export class DocumentStream {
  constructor(player, signal) { this.player = player; this.signal = signal; this.cursor = 0; }
  async open(mimeType) {
    if (!streamTypes().includes(mimeType)) throw Error('HTTPS media codec rejected');
    this.source = new MediaSource();
    this.url = URL.createObjectURL(this.source);
    await completion(this.source, 'sourceopen', this.signal, () => { this.player.preload = 'auto'; this.player.src = this.url; this.player.load(); });
    this.buffer = this.source.addSourceBuffer(mimeType);
  }
  async append(chunks) {
    if (!Array.isArray(chunks) || chunks.length > 32) throw Error('Media batch rejected');
    let bytes = 0;
    for (const chunk of chunks) {
      if (chunk.sequence !== this.cursor + 1 || typeof chunk.data !== 'string' || chunk.data.length > 700000 || !/^[A-Za-z0-9+/]*={0,2}$/.test(chunk.data)) throw Error('Media chunk rejected');
      const body = Uint8Array.from(atob(chunk.data), char => char.charCodeAt(0));
      bytes += body.length;
      if (bytes > 512 * 1024) throw Error('Media batch limit');
      if (this.player.currentTime > 15 && this.buffer.buffered.length && this.buffer.buffered.start(0) < this.player.currentTime - 15) {
        await completion(this.buffer, 'updateend', this.signal, () => this.buffer.remove(0, this.player.currentTime - 10));
      }
      await completion(this.buffer, 'updateend', this.signal, () => this.buffer.appendBuffer(body));
      this.cursor = chunk.sequence;
    }
    this.catchUp();
  }
  catchUp(force = false) {
    const buffered = this.buffer?.buffered;
    if (!buffered) return;
    if (buffered.length) {
      const last = buffered.length - 1, end = buffered.end(last);
      let inRange = false;
      for (let index = 0; index < buffered.length; index++) if (this.player.currentTime >= buffered.start(index) && this.player.currentTime < buffered.end(index) - .02) inRange = true;
      if ((force && end - this.player.currentTime > .75) || end - this.player.currentTime > 2 || (!inRange && end - this.player.currentTime > .1)) this.player.currentTime = Math.max(buffered.start(last), end - .5);
    }
  }
  close() {
    if (this.player.getAttribute('src') === this.url) { this.player.removeAttribute('src'); this.player.load(); }
    if (this.url) URL.revokeObjectURL(this.url);
  }
}
