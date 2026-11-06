import { documentRequest } from './document-request.mjs';
import { DocumentStream, streamTypes, mediaPackets } from './document-stream.mjs';

export class DocumentMediaView {
  constructor(api, report) { this.api = api; this.report = report; this.streams = new Map(); this.failures = new Map(); this.preferHttps = true; this.qualities = new Map(); this.metrics = { bytes: 0, batches: 0, frames: 0, failures: 0, opens: 0, appendMs: 0 }; }
  diagnostics() {
    return { ...this.metrics, streams: [...this.streams.values()].map(entry => {
      const player = entry.player, buffered = player.buffered;
      return { transport: entry.transport, encoding: entry.encoding, ready: player.readyState, paused: player.paused, bufferSeconds: buffered.length ? buffered.end(buffered.length - 1) - player.currentTime : 0, frames: player.getVideoPlaybackQuality?.().totalVideoFrames, droppedFrames: player.getVideoPlaybackQuality?.().droppedVideoFrames, frameAgeMs: entry.frameAt ? Date.now() - entry.frameAt : null };
    }) };
  }
  visible(element) {
    if (document.visibilityState !== 'visible' || element.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) === false) return false;
    for (let current = element; current; current = current.ownerDocument.defaultView.frameElement) {
      const box = current.getBoundingClientRect(), view = current.ownerDocument.defaultView;
      if (box.width <= 0 || box.height <= 0 || box.bottom <= 0 || box.right <= 0 || box.top >= view.innerHeight || box.left >= view.innerWidth) return false;
    }
    return true;
  }
  quality(element) {
    return { width: Math.max(2, Math.min(1920, Math.ceil(element.getBoundingClientRect().width * Math.min(devicePixelRatio || 1, 1.5)))), level: 1 };
  }
  async adapt(entry) {
    if (!this.current(entry)) return;
    try {
      const visible = entry.element.localName === 'audio' || this.visible(entry.element);
      if (entry.visible !== visible) {
        await documentRequest(this.api, 'media', { op: 'visibility', token: entry.token, visible }, entry.controller.signal);
        entry.visible = visible;
      }
      const stats = entry.player.getVideoPlaybackQuality?.(), buffered = entry.player.buffered;
      const frames = (stats?.totalVideoFrames || 0) - (entry.qualityFrames || 0), dropped = (stats?.droppedVideoFrames || 0) - (entry.qualityDropped || 0);
      entry.qualityFrames = stats?.totalVideoFrames || 0; entry.qualityDropped = stats?.droppedVideoFrames || 0;
      const lag = buffered.length ? buffered.end(buffered.length - 1) - entry.player.currentTime : 0;
      const overloaded = visible && !entry.sourcePaused && !entry.blocked && ((frames > 10 && dropped / frames > .15) || lag > 1.5);
      entry.overload = overloaded ? (entry.overload || 0) + 1 : 0;
      const desired = this.quality(entry.element);
      if (Date.now() - entry.startedAt > 15000 && (entry.overload >= 3 && entry.quality.level > 0 || visible && Math.abs(desired.width - entry.quality.width) > Math.max(100, entry.quality.width * .4))) {
        this.qualities.set(entry.key, { ...desired, level: entry.overload >= 3 ? entry.quality.level - 1 : entry.quality.level });
        this.close(entry.key, entry);
        return;
      }
    } catch (error) { if (this.current(entry)) this.fail(entry, error); }
    if (this.current(entry)) entry.adaptation = setTimeout(() => { void this.adapt(entry); }, 2000);
  }
  begin() { this.live = new Set(); }
  current(entry) { return !entry.closed && this.streams.get(entry.key) === entry; }
  ready() { return [...this.streams.values()].every(entry => !entry.blocked && entry.player.readyState >= 2 && !entry.player.paused && (entry.transport === 'https' || entry.peer.connectionState === 'connected')); }
  fail(entry, error, connection = false) {
    if (!this.current(entry)) return;
    if (entry.played && entry.quality?.level > 0 && /timed out|too slow|frames stopped/i.test(error.message)) this.qualities.set(entry.key, { ...entry.quality, level: entry.quality.level - 1 });
    this.metrics.failures++;
    this.close(entry.key, entry);
    if (connection && !entry.played && entry.transport === 'webrtc' && streamTypes().length) {
      this.preferHttps = true;
      this.failures.delete(entry.key);
      this.report(Error('Connecting video/audio over the page connection…'));
      return;
    }
    const attempts = (this.failures.get(entry.key)?.attempts || 0) + 1;
    this.failures.set(entry.key, { attempts, retryAt: Date.now() + Math.min(10000, 500 * 2 ** Math.min(attempts - 1, 5)) });
    this.report(error);
  }
  play(entry, userGesture = false) {
    const player = entry.player;
    if (!this.current(entry) || (!player.srcObject && !entry.stream?.url) || (!userGesture && (!player.paused || entry.playing || entry.playRetry || entry.blocked))) return;
    const attempt = entry.playAttempt = (entry.playAttempt || 0) + 1;
    entry.playing = true;
    const failed = error => {
      if (!this.current(entry) || attempt !== entry.playAttempt) return;
      if (error.name === 'AbortError' && (entry.playAborts = (entry.playAborts || 0) + 1) <= 3) {
        entry.playRetry = setTimeout(() => { entry.playRetry = null; this.play(entry); }, 200);
      } else if (error.name === 'NotAllowedError') {
        entry.blocked = true;
        this.report(Error('Tap the video to start playback on this phone, or tap Enable page sound.'));
      } else this.fail(entry, error);
    };
    try {
      Promise.resolve(player.play()).then(() => {
        if (!this.current(entry) || attempt !== entry.playAttempt) return;
        entry.playAborts = 0;
        clearTimeout(entry.deadline);
        this.failures.delete(entry.key);
      }, failed).finally(() => { if (attempt === entry.playAttempt) entry.playing = false; });
    } catch (error) { entry.playing = false; failed(error); }
  }
  activate(event) {
    for (const entry of this.streams.values()) {
      if (!['video', 'audio'].includes(entry.element.localName) || entry.element.ownerDocument !== event.target.ownerDocument || entry.sourcePaused !== false) continue;
      if (entry.element.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) === false) continue;
      const box = entry.element.getBoundingClientRect();
      if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) continue;
      if (!entry.blocked && !entry.player.paused && entry.played && (!entry.player.requestVideoFrameCallback || Date.now() - entry.frameAt < 2000)) return false;
      entry.blocked = false;
      entry.stream?.catchUp(true);
      this.play(entry, true);
      return true;
    }
    return false;
  }
  draw(entry) {
    if (!entry.player.requestVideoFrameCallback || entry.drawing) return;
    entry.drawing = true;
    const draw = () => {
      if (!this.current(entry)) return;
      try {
        entry.frameAt = Date.now();
        this.metrics.frames++;
        if (entry.player !== entry.element) entry.element.getContext('2d').drawImage(entry.player, 0, 0, entry.element.width, entry.element.height);
        entry.frame = entry.player.requestVideoFrameCallback(draw);
      } catch (error) { this.fail(entry, error); }
    };
    entry.frame = entry.player.requestVideoFrameCallback(draw);
  }
  progress(entry, playback) {
    if (!playback || typeof playback.paused !== 'boolean' || !Number.isFinite(playback.time)) return;
    const now = Date.now();
    entry.sourcePaused = playback.paused;
    const advancing = Number.isFinite(playback.frames) ? playback.frames !== (entry.sourceFrames ?? playback.frames) : Math.abs(playback.time - (entry.sourceTime ?? playback.time)) > .02;
    entry.sourceTime = playback.time;
    entry.sourceFrames = playback.frames;
    if (!advancing || playback.paused || entry.blocked || entry.element.localName !== 'video' || document.visibilityState !== 'visible') { entry.progressAt = now; return; }
    if (entry.element.checkVisibility?.({ checkOpacity: true, checkVisibilityCSS: true }) === false) { entry.progressAt = now; return; }
    for (let element = entry.element; element; element = element.ownerDocument.defaultView.frameElement) {
      const box = element.getBoundingClientRect(), view = element.ownerDocument.defaultView, style = view.getComputedStyle(element);
      if (box.width <= 0 || box.height <= 0 || box.bottom <= 0 || box.right <= 0 || box.top >= view.innerHeight || box.left >= view.innerWidth || style.visibility !== 'visible' || style.opacity === '0') { entry.progressAt = now; return; }
    }
    if (now - Math.max(entry.frameAt || 0, entry.progressAt || entry.startedAt) > 8000) this.fail(entry, Error('Video frames stopped arriving. Reconnecting the video…'));
  }
  attach(node, element, generation, frames) {
    const key = `${generation}:${node.id}`;
    this.live.add(key);
    const existing = this.streams.get(key);
    if (existing?.element === element) { existing.sourcePaused = node.mediaPaused; this.play(existing); return; }
    if (existing) this.close(key, existing);
    if (node.mediaReady === false || Date.now() < (this.failures.get(key)?.retryAt || 0)) return;
    if (this.streams.size >= 8) return;
    let entry;
    try {
      const transport = this.preferHttps && streamTypes().length ? 'https' : 'webrtc';
      entry = { key, transport, startedAt: Date.now(), sourcePaused: node.mediaPaused, controller: new AbortController(), closed: false, element };
      this.streams.set(key, entry);
      this.metrics.opens++;
      const player = node.tag === 'canvas' ? document.createElement('video') : element;
      entry.player = player;
      if (node.tag === 'canvas') {
        player.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none';
        player.setAttribute('aria-hidden', 'true');
        document.body.append(player);
      }
      entry.deadline = setTimeout(() => {
        if (player.readyState < 2 || (!entry.blocked && player.paused) || (entry.peer && entry.peer.connectionState !== 'connected')) this.fail(entry, Error('Media connection timed out'), true);
      }, transport === 'https' ? 15000 : 6000);
      entry.onPlaying = () => {
        if (!this.current(entry)) return;
        entry.played = true;
        clearTimeout(entry.deadline);
        entry.playAborts = 0;
        this.failures.delete(key);
      };
      player.addEventListener?.('playing', entry.onPlaying);
      player.muted = !this.soundEnabled;
      player.autoplay = true;
      player.playsInline = true;
      if (transport === 'https') {
        void this.openStream(entry, node, generation, frames).catch(error => this.fail(entry, error));
        return;
      }
      entry.peer = new RTCPeerConnection({ iceServers: [] });
      entry.peer.onconnectionstatechange = () => {
        if (!this.current(entry)) return;
        const state = entry.peer.connectionState;
        if (state === 'failed' || state === 'closed') this.fail(entry, Error('Media connection lost'), true);
        else if (state === 'disconnected' && !entry.disconnected) entry.disconnected = setTimeout(() => this.fail(entry, Error('Media connection lost'), true), 3000);
        else if (state === 'connected') { entry.connected = true; clearTimeout(entry.disconnected); entry.disconnected = null; }
      };
      entry.peer.ontrack = event => {
        if (!this.current(entry)) return;
        try {
          const stream = event.streams[0];
          if (!stream) throw Error('Media stream missing');
          if (player.srcObject !== stream) player.srcObject = stream;
          event.track?.addEventListener('ended', () => this.fail(entry, Error('Media track ended')), { once: true });
          this.play(entry);
          this.draw(entry);
        } catch (error) { this.fail(entry, error); }
      };
      void this.open(entry, node, generation, frames).catch(error => this.fail(entry, error));
    } catch (error) {
      if (entry) this.fail(entry, error, true);
      else { this.failures.set(key, { attempts: 1, retryAt: Date.now() + 10000 }); this.report(error); }
    }
  }
  async openStream(entry, node, generation, frames) {
    const request = data => documentRequest(this.api, 'media', data, entry.controller.signal);
    entry.quality = this.qualities.get(entry.key) || this.quality(entry.element);
    const result = await request({ op: 'open', transport: 'https', types: streamTypes(), generation, node: node.id, frames, quality: entry.quality });
    entry.encoding = result.encoding;
    entry.token = result.token;
    if (!this.current(entry)) { this.release(entry.token); return; }
    entry.stream = new DocumentStream(entry.player, entry.controller.signal);
    const opening = entry.stream.open(result.mimeType);
    this.play(entry);
    await opening;
    if (!this.current(entry)) return;
    this.draw(entry);
    const receive = async ({ chunks, playback }) => {
      if (!this.current(entry)) return;
      this.progress(entry, playback);
      if (!this.current(entry)) return;
      const started = performance.now();
      await entry.stream.append(chunks);
      this.metrics.appendMs += performance.now() - started;
      this.metrics.batches++;
      this.metrics.bytes += chunks.reduce((total, chunk) => total + (chunk.data instanceof Uint8Array ? chunk.data.length : Math.floor(chunk.data.length * 3 / 4) - (chunk.data.endsWith('==') ? 2 : chunk.data.endsWith('=') ? 1 : 0)), 0);
      if (this.current(entry)) this.play(entry);
    };
    if (result.encoding) { entry.visible = true; entry.adaptation = setTimeout(() => { void this.adapt(entry); }, 2000); }
    if (result.delivery === 'binary') {
      const response = await request({ op: 'stream', token: entry.token });
      for await (const packet of mediaPackets(response, generation, entry.controller.signal)) {
        if (!this.current(entry)) break;
        await receive(packet);
      }
      return;
    }
    const poll = async () => {
      if (!this.current(entry)) return;
      try {
        await receive(await request({ op: 'read', token: entry.token, after: entry.stream.cursor }));
        if (this.current(entry)) entry.timer = setTimeout(poll, 150);
      } catch (error) { this.fail(entry, error); }
    };
    void poll();
  }

  async open(entry, node, generation, frames) {
    const request = data => documentRequest(this.api, 'media', data, entry.controller.signal);
    const result = await request({ op: 'open', generation, node: node.id, frames });
    entry.token = result.token;
    if (!this.current(entry)) { this.release(entry.token); return; }
    await entry.peer.setRemoteDescription(result.offer);
    if (!this.current(entry)) return;
    const answer = await entry.peer.createAnswer();
    if (!this.current(entry)) return;
    await entry.peer.setLocalDescription(answer);
    if (!this.current(entry)) return;
    await new Promise(resolve => {
      const done = () => {
        clearTimeout(timer);
        entry.peer.removeEventListener('icegatheringstatechange', changed);
        entry.controller.signal.removeEventListener('abort', done);
        resolve();
      };
      const changed = () => { if (entry.peer.iceGatheringState === 'complete') done(); };
      const timer = setTimeout(done, 2000);
      entry.peer.addEventListener('icegatheringstatechange', changed);
      entry.controller.signal.addEventListener('abort', done, { once: true });
      changed();
    });
    if (!this.current(entry)) return;
    await request({ op: 'answer', token: entry.token, answer: entry.peer.localDescription.toJSON() });
    const renew = async () => {
      if (!this.current(entry)) return;
      try { await request({ op: 'renew', token: entry.token }); }
      catch (error) { this.fail(entry, error); return; }
      if (this.current(entry)) entry.timer = setTimeout(renew, 3000);
    };
    if (this.current(entry)) entry.timer = setTimeout(renew, 3000);
  }
  finish() {
    for (const key of this.streams.keys()) if (!this.live.has(key)) this.close(key);
    for (const key of this.qualities.keys()) if (!this.live.has(key)) this.qualities.delete(key);
    for (const key of this.failures.keys()) if (!this.live.has(key)) this.failures.delete(key);
  }
  enableSound() {
    this.soundEnabled = true;
    for (const entry of this.streams.values()) { entry.player.muted = false; entry.blocked = false; entry.stream?.catchUp(true); this.play(entry, true); }
  }
  release(token) { void documentRequest(this.api, 'media', { op: 'close', token }, undefined, 3000).catch(() => {}); }
  close(key, entry = this.streams.get(key)) {
    if (!entry || !this.current(entry)) return;
    entry.closed = true;
    this.streams.delete(key);
    entry.controller.abort();
    for (const timer of ['timer', 'deadline', 'playRetry', 'disconnected', 'adaptation']) clearTimeout(entry[timer]);
    if (entry.frame !== undefined) entry.player?.cancelVideoFrameCallback(entry.frame);
    entry.peer?.close();
    entry.stream?.close();
    if (entry.player) {
      entry.player.removeEventListener?.('playing', entry.onPlaying);
      entry.player.srcObject = null;
      if (entry.player !== entry.element) entry.player.remove();
    }
    if (entry.token) this.release(entry.token);
  }
  dispose() { for (const key of this.streams.keys()) this.close(key); this.failures.clear(); this.qualities.clear(); }
}
