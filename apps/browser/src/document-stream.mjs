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
      const binary = chunk.data instanceof Uint8Array;
      if (chunk.sequence !== this.cursor + 1 || (!binary && (typeof chunk.data !== 'string' || chunk.data.length > 700000 || !/^[A-Za-z0-9+/]*={0,2}$/.test(chunk.data)))) throw Error('Media chunk rejected');
      const body = binary ? chunk.data : Uint8Array.from(atob(chunk.data), char => char.charCodeAt(0));
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

export async function* mediaPackets(response, generation, signal) {
  const reader = response.body.getReader();
  let pending = new Uint8Array(), offset = 0;
  const cancelled = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener('abort', cancelled, { once: true });
  const exact = async size => {
    const result = new Uint8Array(size);
    for (let written = 0; written < size;) {
      signal.throwIfAborted();
      if (offset === pending.length) {
        let timer;
        try {
          const value = await Promise.race([reader.read(), new Promise((_, reject) => { timer = setTimeout(() => reject(Error('Media stream timed out')), 8000); })]);
          if (value.done) throw Error('Media stream ended');
          pending = value.value; offset = 0;
        } finally { clearTimeout(timer); }
      }
      const count = Math.min(size - written, pending.length - offset);
      result.set(pending.subarray(offset, offset + count), written); offset += count; written += count;
    }
    return result;
  };
  try {
    while (true) {
      const length = new DataView((await exact(4)).buffer).getUint32(0);
      if (length > 16384 || !length) throw Error('Media header rejected');
      const packet = JSON.parse(new TextDecoder().decode(await exact(length)));
      if (packet.generation !== generation || !Array.isArray(packet.chunks) || packet.chunks.length > 32) throw Error('Media scope rejected');
      let total = 0;
      for (const chunk of packet.chunks) {
        if (!Number.isSafeInteger(chunk.bytes) || chunk.bytes < 1 || (total += chunk.bytes) > 512 * 1024) throw Error('Media batch limit');
      }
      for (const chunk of packet.chunks) chunk.data = await exact(chunk.bytes);
      yield packet;
    }
  } finally {
    signal.removeEventListener('abort', cancelled);
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}
