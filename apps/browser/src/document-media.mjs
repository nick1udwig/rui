import { documentRequest } from './document-request.mjs';

export class DocumentMediaView {
  constructor(api, report) { this.api = api; this.report = report; this.streams = new Map(); this.failures = new Map(); }
  begin() { this.live = new Set(); }
  current(entry) { return !entry.closed && this.streams.get(entry.key) === entry; }
  fail(entry, error) {
    if (!this.current(entry)) return;
    this.close(entry.key, entry);
    const attempts = (this.failures.get(entry.key)?.attempts || 0) + 1;
    this.failures.set(entry.key, { attempts, retryAt: Date.now() + Math.min(10000, 500 * 2 ** Math.min(attempts - 1, 5)) });
    this.report(error);
  }
  play(entry) {
    const player = entry.player;
    if (!this.current(entry) || !player.srcObject || !player.paused || entry.playing || entry.playRetry || entry.blocked) return;
    entry.playing = true;
    const failed = error => {
      if (!this.current(entry)) return;
      if (error.name === 'AbortError' && (entry.playAborts = (entry.playAborts || 0) + 1) <= 3) {
        entry.playRetry = setTimeout(() => { entry.playRetry = null; this.play(entry); }, 200);
      } else if (error.name === 'NotAllowedError') {
        entry.blocked = true;
        this.report(Error('Tap Enable page sound to start playback.'));
      } else this.fail(entry, error);
    };
    try {
      Promise.resolve(player.play()).then(() => {
        if (!this.current(entry)) return;
        entry.playAborts = 0;
        clearTimeout(entry.deadline);
        this.failures.delete(entry.key);
      }, failed).finally(() => { entry.playing = false; });
    } catch (error) { entry.playing = false; failed(error); }
  }
  attach(node, element, generation, frames) {
    const key = `${generation}:${node.id}`;
    this.live.add(key);
    const existing = this.streams.get(key);
    if (existing?.element === element) { this.play(existing); return; }
    if (existing) this.close(key, existing);
    if (node.mediaReady === false || Date.now() < (this.failures.get(key)?.retryAt || 0)) return;
    if (this.streams.size >= 8) return;
    let entry;
    try {
      entry = { key, peer: new RTCPeerConnection({ iceServers: [] }), controller: new AbortController(), closed: false, element };
      this.streams.set(key, entry);
      const player = node.tag === 'canvas' ? document.createElement('video') : element;
      entry.player = player;
      if (node.tag === 'canvas') {
        player.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none';
        player.setAttribute('aria-hidden', 'true');
        document.body.append(player);
      }
      entry.deadline = setTimeout(() => {
        if (!entry.blocked && (player.readyState < 2 || player.paused || entry.peer.connectionState !== 'connected')) this.fail(entry, Error('Media connection timed out'));
      }, 10000);
      entry.onPlaying = () => {
        if (!this.current(entry)) return;
        clearTimeout(entry.deadline);
        entry.playAborts = 0;
        this.failures.delete(key);
      };
      player.addEventListener?.('playing', entry.onPlaying);
      player.muted = !this.soundEnabled;
      player.autoplay = true;
      player.playsInline = true;
      entry.peer.onconnectionstatechange = () => {
        if (!this.current(entry)) return;
        const state = entry.peer.connectionState;
        if (state === 'failed' || state === 'closed') this.fail(entry, Error('Media connection lost'));
        else if (state === 'disconnected' && !entry.disconnected) entry.disconnected = setTimeout(() => this.fail(entry, Error('Media connection lost')), 3000);
        else if (state === 'connected') { clearTimeout(entry.disconnected); entry.disconnected = null; }
      };
      entry.peer.ontrack = event => {
        if (!this.current(entry)) return;
        try {
          const stream = event.streams[0];
          if (!stream) throw Error('Media stream missing');
          if (player.srcObject !== stream) player.srcObject = stream;
          event.track?.addEventListener('ended', () => this.fail(entry, Error('Media track ended')), { once: true });
          this.play(entry);
          if (node.tag === 'canvas' && !entry.drawing) {
            entry.drawing = true;
            const draw = () => {
              if (!this.current(entry)) return;
              try {
                if (player.readyState >= 2) element.getContext('2d').drawImage(player, 0, 0, element.width, element.height);
                entry.frame = player.requestVideoFrameCallback(draw);
              } catch (error) { this.fail(entry, error); }
            };
            draw();
          }
        } catch (error) { this.fail(entry, error); }
      };
      void this.open(entry, node, generation, frames).catch(error => this.fail(entry, error));
    } catch (error) {
      if (entry) this.fail(entry, error);
      else { this.failures.set(key, { attempts: 1, retryAt: Date.now() + 10000 }); this.report(error); }
    }
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
    for (const key of this.failures.keys()) if (!this.live.has(key)) this.failures.delete(key);
  }
  enableSound() {
    this.soundEnabled = true;
    for (const entry of this.streams.values()) { entry.player.muted = false; entry.blocked = false; this.play(entry); }
  }
  release(token) { void documentRequest(this.api, 'media', { op: 'close', token }, undefined, 3000).catch(() => {}); }
  close(key, entry = this.streams.get(key)) {
    if (!entry || !this.current(entry)) return;
    entry.closed = true;
    this.streams.delete(key);
    entry.controller.abort();
    for (const timer of ['timer', 'deadline', 'playRetry', 'disconnected']) clearTimeout(entry[timer]);
    if (entry.frame !== undefined) entry.player?.cancelVideoFrameCallback(entry.frame);
    entry.peer.close();
    if (entry.player) {
      entry.player.removeEventListener?.('playing', entry.onPlaying);
      entry.player.srcObject = null;
      if (entry.player !== entry.element) entry.player.remove();
    }
    if (entry.token) this.release(entry.token);
  }
  dispose() { for (const key of this.streams.keys()) this.close(key); this.failures.clear(); }
}
