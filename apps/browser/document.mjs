import { randomBytes, createHash } from "node:crypto";
import { DocumentMedia } from "./document-media.mjs";
import { concurrent, taskPool } from "./src/document-work.mjs";

const MAX_BYTES = 8 * 1024 * 1024;

// Runs in the source document. IDs survive observations and most same-document navigation, but never a document replacement.
function observeDocument({ key, knownStyles, generation, reuse, established }) {
  const previous = globalThis[key];
  // A same-document route change after heavy identity use starts a fresh identity budget.
  if (established && (previous?.generation !== generation || (previous.url !== location.href && previous.next > 50000))) return JSON.stringify({ replaced: true });
  const state = globalThis[key] ||= {};
  state.url = location.href;
  if (state.generation !== generation) {
    state.observer?.disconnect();
    for (const [type, listener] of state.listeners || []) document.removeEventListener(type, listener, true);
    Object.assign(state, { generation, hasFrames: false, nodes: new WeakMap(), reverse: new Map(), next: 1, styles: new WeakMap(), nextStyle: 1, scrollers: new WeakSet() });
  }
  if (!state.cache || state.cacheGeneration !== generation) {
    state.cacheGeneration = generation; state.cache = new WeakMap(); state.origins = new WeakMap(); state.dirty = new WeakSet(); state.fullAt = 0;
    state.version = 0; state.waiters = new Set();
    const invalidate = node => { state.version++; for (const wake of state.waiters) wake(); if (node === document) node = document.documentElement; for (let current = node; current; current = current.parentNode || current.host) state.dirty.add(current); };
    state.observer = new MutationObserver(records => { for (const record of records) invalidate(record.target); });
    state.observer.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    state.listeners = ['input', 'change', 'scroll', 'play', 'pause', 'loadeddata', 'load', 'emptied', 'focusin', 'focusout'].map(type => {
      const listener = event => invalidate(event.target); document.addEventListener(type, listener, true); return [type, listener];
    });
  }
  const full = performance.now() - state.fullAt > 2000;
  const id = (node) => {
    if (!state.nodes.has(node)) {
      if (state.next > 100000) throw Error("Source identity limit");
      state.nodes.set(node, state.next++);
    }
    return state.nodes.get(node);
  };
  const originals = state.origins, sizes = new Map();
  state.scrollers ||= new WeakSet();
  let count = 0, visited = 0;
  const visit = (node, depth = 0, shadow = false) => {
    const cached = state.cache.get(node);
    if (!full && cached && !state.dirty.has(node)) { count += cached.size; sizes.set(cached.record, cached.size); return cached.record; }
    const before = count;
    const record = inspect(node, depth, shadow);
    if (record) state.cache.set(node, { record, size: count - before });
    return record;
  };
  const inspect = (node, depth = 0, shadow = false) => {
    if (++visited > 150000 || depth > 128) throw Error("Document scan limit");
    if (node.nodeType === 3) { count++; return { id: id(node), text: node.data }; }
    if (node.nodeType !== 1) return null;
    const tag = node.localName;
    if (["script", "noscript", "base", "meta", "link"].includes(tag) || (tag === "style" && !shadow)) return null;
    count++;
    const attrs = Object.fromEntries([...node.attributes].filter(a => !/^on/i.test(a.name) && !["srcdoc", "action", "formaction", "nonce"].includes(a.name)).map(a => [a.name, a.value]));
    if (tag === "img") attrs.src = node.currentSrc || node.src;
    if (tag === "video" && node.poster) attrs.poster = node.poster;
    const result = { id: id(node), tag, ns: node.namespaceURI, attrs, children: [] };
    originals.set(result, node);
    if (node.scrollLeft || node.scrollTop) state.scrollers.add(node);
    if (state.scrollers.has(node)) result.scroll = { x: node.scrollLeft, y: node.scrollTop };
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement || node instanceof HTMLSelectElement) {
      result.value = node.type === "password" ? "" : node.value;
      result.checked = !!node.checked;
      if (node.type === "password") delete attrs.value;
    }
    if (node.isContentEditable && !node.parentElement?.isContentEditable) result.value = node.textContent;
    if (tag === "iframe") state.hasFrames = true;
    if (["canvas", "video", "audio", "iframe", "object", "embed"].includes(tag)) {
      if (typeof node.captureStream === "function") { result.media = true; result.mediaReady = tag === "canvas" || node.readyState >= 2; }
      if (tag === 'video' || tag === 'audio') result.mediaPaused = node.paused;
      return result;
    }
    result.children = [...node.childNodes].map(n => visit(n, depth + 1, shadow)).filter(Boolean);
    if (node.shadowRoot) {
      state.observer.observe(node.shadowRoot, { subtree: true, childList: true, attributes: true, characterData: true });
      result.shadow = [...node.shadowRoot.childNodes].map(n => visit(n, depth + 1, true)).filter(Boolean);
      result.shadowStyles = [...node.shadowRoot.adoptedStyleSheets].map(sheet => [...sheet.cssRules].map(rule => rule.cssText).join("\n"));
    }
    sizes.set(result, 1 + [...result.children, ...(result.shadow || [])].reduce((sum, child) => sum + (sizes.get(child) || 1), 0));
    return result;
  };
  const styles = [];
  state.styles ||= new WeakMap();
  state.nextStyle ||= 1;
  const known = new Map(knownStyles);
  for (const sheet of document.styleSheets) {
    let text, url;
    const base = sheet.href || document.baseURI;
    try { text = [...sheet.cssRules].map(rule => rule.cssText).join("\n"); }
    catch { url = sheet.href; }
    let entry = state.styles.get(sheet);
    if (!entry) { entry = { id: state.nextStyle++, version: 0 }; state.styles.set(sheet, entry); }
    if (entry.version === 0 || text !== entry.text || url !== entry.url || base !== entry.base) {
      Object.assign(entry, { text, url, base, version: entry.version + 1 });
    }
    styles.push(known.get(entry.id) === entry.version ? { id: entry.id, version: entry.version } : { ...entry });
  }
  const root = visit(document.documentElement);
  let deferredNodes = 0;
  if (count > 20000) {
    state.cache = new WeakMap();
    const eligible = new Map();
    const top = -innerHeight * 2, bottom = innerHeight * 3, left = -innerWidth, right = innerWidth * 2;
    const measure = (record, inHead = false) => {
      inHead ||= record.tag === 'head';
      const childVisible = [...(record.children || []), ...(record.shadow || [])].map(child => measure(child, inHead)).some(Boolean);
      const element = originals.get(record);
      if (!element || inHead) return childVisible;
      const box = element.getBoundingClientRect();
      const visible = childVisible || (box.width > 0 && box.height > 0 && box.bottom >= top && box.top <= bottom && box.right >= left && box.left <= right);
      if (!visible && !['html', 'body', 'style'].includes(record.tag) && record.ns === 'http://www.w3.org/1999/xhtml' && (sizes.get(record) || 1) > 1) {
        const style = getComputedStyle(element);
        if (style.display === 'none' || (box.width > 0 && box.height > 0 && style.transform === 'none' && [style.width, style.height].every(value => /^\d+(?:\.\d+)?px$/.test(value)))) eligible.set(record, {
          hidden: style.display === 'none', width: style.width, height: style.height,
          distance: Math.max(top - box.bottom, box.top - bottom, left - box.right, box.left - right),
        });
      }
      return visible;
    };
    measure(root);
    const candidates = [];
    const collect = record => {
      if (eligible.has(record)) candidates.push(record);
      else for (const child of [...(record.children || []), ...(record.shadow || [])]) collect(child);
    };
    collect(root);
    candidates.sort((a, b) => Number(eligible.get(b).hidden) - Number(eligible.get(a).hidden) || eligible.get(b).distance - eligible.get(a).distance || sizes.get(b) - sizes.get(a));
    for (const record of candidates) {
      if (count <= 18000) break;
      const { hidden, width, height } = eligible.get(record);
      record.deferred = { hidden, width, height };
      record.children = [];
      if (record.shadow) { record.shadow = []; record.shadowStyles = []; }
      const removed = sizes.get(record) - 1;
      count -= removed;
      deferredNodes += removed;
    }
    if (count > 20000) throw Error('Visible document limit');
  }
  const styleKey = JSON.stringify(styles.map(style => [style.id, style.version]));
  const unchanged = reuse && !state.hasFrames && state.lastRoot === root && state.lastStyles === styleKey && state.width === innerWidth && state.height === innerHeight && state.base === document.baseURI;
  state.lastRoot = root; state.lastStyles = styleKey; state.width = innerWidth; state.height = innerHeight; state.base = document.baseURI;
  state.dirty = new WeakSet(); if (full) state.fullAt = performance.now();
  if (unchanged) return JSON.stringify({ unchanged: true, sourceVersion: state.version, inspectedNodes: visited });
  const reverse = new Map(), regions = [];
  const retain = record => {
    const element = originals.get(record);
    if (element) reverse.set(record.id, element);
    if (["canvas", "video", "audio", "iframe", "object", "embed"].includes(record.tag) && !record.media) regions.push({ id: record.id, kind: record.tag });
    for (const child of [...(record.children || []), ...(record.shadow || [])]) retain(child);
  };
  retain(root);
  state.reverse = reverse;
  // Avoid Playwright's per-property encoding for this large, JSON-only snapshot.
  return JSON.stringify({ sourceVersion: state.version, inspectedNodes: visited, root, styles, unsupported: regions, deferredNodes, base: document.baseURI, width: innerWidth, height: innerHeight });
}

export class DocumentActionRejected extends Error {}

export class BrowserDocument {
  constructor(page, preserveWindow = false) {
    this.preserveWindow = preserveWindow;
    this.frame = typeof page.mainFrame === "function" ? page.mainFrame() : page;
    this.page = this.frame.page();
    this.frames = new Map();
    this.media = new DocumentMedia(this);
    this.key = `__rui_document_${randomBytes(16).toString("hex")}`;
    this.generation = randomBytes(16).toString("hex");
    this.revision = 0;
    this.assets = new Map();
    this.resources = new Map();
    this.styles = new Map();
    this.rewrites = new Map();
    this.sourceStyles = new Map();
    this.fetchAsset = taskPool(8);
    this.failedAssets = new Map();
    // History API and fragment changes also emit framenavigated; only a replaced document needs a new generation.
    // Capture also detects replacement, so this early check only releases old media promptly.
    this.navigation = (frame) => {
      if (frame !== this.frame) return;
      const generation = this.generation;
      void this.frame.evaluate(key => key in globalThis, this.key)
        .catch(() => false)
        .then(current => { if (!current && generation === this.generation && this.lastCapture?.generation === generation) this.replaceDocument(); });
    };
    this.page.on("framenavigated", this.navigation);
  }
  replaceDocument() {
    void this.media.dispose();
    this.generation = randomBytes(16).toString("hex");
    this.assets.clear();
    this.resources.clear();
    this.styles.clear();
    this.rewrites.clear();
    this.sourceStyles.clear();
    this.failedAssets.clear();
  }
  dispose() {
    if (this.disposal) return this.disposal;
    this.disposed = true;
    this.page.off("framenavigated", this.navigation);
    this.disposal = (async () => {
      await this.pendingCapture?.catch(() => {});
      await Promise.all([this.restoreViewport(), this.media.dispose(), ...[...this.frames.values()].map(child => child.dispose())]);
      await this.frame.evaluate(key => {
        const state = globalThis[key];
        if (!state) return;
        state.observer?.disconnect();
        for (const [type, listener] of state.listeners || []) document.removeEventListener(type, listener, true);
        for (const wake of state.waiters || []) wake();
        state.waiters?.clear();
        for (const session of state.media?.values() || []) session.close();
        state.reverse?.clear();
        state.lastRoot = null;
        delete globalThis[key];
      }, this.key).catch(() => {});
      this.lastCapture = null;
      this.assets.clear();
      this.resources.clear();
      this.styles.clear();
      this.rewrites.clear();
      this.sourceStyles.clear();
      this.failedAssets.clear();
      this.frames.clear();
    })();
    return this.disposal;
  }
  async restoreViewport() {
    const session = this.viewportSession;
    this.viewportSession = null;
    if (session) {
      await session.send('Emulation.clearDeviceMetricsOverride').catch(() => {});
      await session.detach().catch(() => {});
    }
  }
  routed(frames = []) {
    if (frames.length > 16) throw Error("Frame limit");
    let source = this;
    for (const route of frames) {
      if (route.generation !== source.generation || !source.frames.has(route.node)) throw Error("Stale frame route");
      source = source.frames.get(route.node);
    }
    return source;
  }
  resource(key) {
    const body = this.resources.get(key) ?? this.styles.get(key);
    if (body !== undefined) return body;
    for (const child of this.frames.values()) { const value = child.resource(key); if (value !== undefined) return value; }
  }
  async dispatch(command) {
    if (this.disposed) throw new DocumentActionRejected("Document detached");
    if (command.frames?.length) {
      const [route, ...frames] = command.frames;
      if (command.frames.length > 16 || route.generation !== this.generation || !this.frames.has(route.node)) throw new DocumentActionRejected("Stale frame route");
      return this.frames.get(route.node).dispatch({ ...command, frames });
    }
    if (command.generation !== this.generation || !Number.isSafeInteger(command.revision) || command.revision < 1 || command.revision > this.revision) throw new DocumentActionRejected("Stale document");
    if (command.action === "resize") {
      const { width, height } = command;
      if (![width, height].every(n => Number.isInteger(n) && n >= 240 && n <= 2560)) throw new DocumentActionRejected("Viewport limit");
      if (this.preserveWindow) {
        this.viewportSession ||= await this.page.context().newCDPSession(this.page);
        await this.viewportSession.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 0, mobile: false, dontSetVisibleSize: true });
      } else await this.page.setViewportSize({ width, height });
      this.viewportChanged = true;
      return;
    }
    if (!Number.isSafeInteger(command.node)) throw new DocumentActionRejected("Invalid document target");
    const handle = await this.frame.evaluateHandle(({ key, id, generation }) => globalThis[key]?.generation === generation ? globalThis[key].reverse.get(id) : undefined, { key: this.key, id: command.node, generation: this.generation });
    try {
      const element = handle.asElement();
      if (!element || !await element.evaluate(n => n.isConnected && !n.closest('[inert]') && !n.disabled)) throw new DocumentActionRejected("Unavailable document target");
      await this.page.bringToFront();
      if (command.action === "click") {
        let position;
        if (command.position) {
          if (![command.position.x, command.position.y].every(n => Number.isFinite(n) && n >= 0 && n <= 1)) throw new DocumentActionRejected("Pointer position rejected");
          const box = await element.boundingBox();
          if (!box) throw new DocumentActionRejected("Invisible target");
          position = { x: command.position.x * box.width, y: command.position.y * box.height };
        }
        try { await element.click({ trial: true, timeout: 1000, position }); }
        catch { throw new DocumentActionRejected("Target is no longer clickable. Wait for the page update and try again."); }
        await element.click({ timeout: 1000, position, noWaitAfter: true });
      }
      else if (command.action === "drag") {
        const points = command.points;
        if (!Array.isArray(points) || points.length < 2 || points.length > 64 || !points.every(p => [p.x, p.y].every(n => Number.isFinite(n) && n >= 0 && n <= 1))) throw new DocumentActionRejected("Drag path rejected");
        await element.scrollIntoViewIfNeeded({ timeout: 1000 });
        if (!await element.evaluate((n, point) => {
          const box = n.getBoundingClientRect();
          const root = n.getRootNode();
          const hit = root.elementFromPoint(box.left + box.width * point.x, box.top + box.height * point.y);
          return hit === n || n.contains(hit);
        }, points[0])) throw new DocumentActionRejected("Drag target obscured");
        const box = await element.boundingBox();
        if (!box) throw new DocumentActionRejected("Invisible drag target");
        const position = point => [box.x + box.width * point.x, box.y + box.height * point.y];
        await this.page.mouse.move(...position(points[0]));
        await this.page.mouse.down();
        try { for (const point of points.slice(1)) await this.page.mouse.move(...position(point)); }
        finally { await this.page.mouse.up(); }
      }
      else if (command.action === "value") {
        if (typeof command.value !== "string" || command.value.length > 16384) throw new DocumentActionRejected("Text limit");
        if (command.key !== undefined && command.key !== "Enter") throw new DocumentActionRejected("Unsupported submit key");
        const valid = await element.evaluate((n, expected) => n.type !== 'password' && String(n.value ?? '') === expected, command.expected);
        if (!valid) throw new DocumentActionRejected("Source text changed");
        if (await element.evaluate(n => n.localName === 'select')) await element.selectOption(command.value, { timeout: 1000 });
        else await element.fill(command.value, { timeout: 1000 });
        if (command.key) await element.press(command.key, { timeout: 1000 });
      } else if (command.action === "rich-value") {
        if (typeof command.value !== 'string' || command.value.length > 16384 || typeof command.expected !== 'string') throw new DocumentActionRejected("Text limit");
        const edit = await element.evaluate((n, command) => {
          if (!n.isContentEditable || n.textContent !== command.expected) return null;
          if (command.expected === command.value) return { noop: true };
          const before = command.expected, after = command.value;
          let start = 0, tail = 0;
          while (start < before.length && start < after.length && before[start] === after[start]) start++;
          if (start && /[\uD800-\uDBFF]/.test(before[start - 1])) start--;
          while (tail < before.length - start && tail < after.length - start && before[before.length - tail - 1] === after[after.length - tail - 1]) tail++;
          if (tail && /[\uDC00-\uDFFF]/.test(before[before.length - tail])) tail--;
          const boundary = offset => {
            const walker = document.createTreeWalker(n, NodeFilter.SHOW_TEXT);
            let text;
            while ((text = walker.nextNode())) { if (offset <= text.length) return [text, offset]; offset -= text.length; }
            return [n, n.childNodes.length];
          };
          n.focus();
          const range = document.createRange(); range.setStart(...boundary(start)); range.setEnd(...boundary(before.length - tail));
          const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range);
          return { text: after.slice(start, after.length - tail) };
        }, command);
        if (!edit) throw new DocumentActionRejected("Source text changed");
        if (edit.noop) return;
        if (edit.text) await this.page.keyboard.insertText(edit.text);
        else await this.page.keyboard.press('Backspace');
      } else if (command.action === "scroll") {
        if (![command.x, command.y].every(n => Number.isFinite(n) && Math.abs(n) <= 10000000)) throw new DocumentActionRejected("Scroll limit");
        await element.evaluate((n, c) => n.scrollTo(c.x, c.y), command);
      } else throw new DocumentActionRejected("Unsupported document action");
    } finally { await handle.dispose(); }
  }
  async asset(url, generation = this.generation) {
    if (generation !== this.generation) throw Error("Navigation during resource capture");
    const intern = data => {
      if (generation !== this.generation) throw Error("Navigation during resource capture");
      if (data.length > MAX_BYTES) throw Error("Resource limit");
      const key = createHash('sha256').update(data).digest('hex');
      if (!this.resources.has(key)) {
        if ([...this.resources.values()].reduce((n, v) => n + v.length, data.length) > MAX_BYTES) throw Error("Resource cache limit");
        this.resources.set(key, data);
      }
      return `rui-resource:${key}`;
    };
    if (!/^https?:/.test(url)) return /^data:(image\/(png|jpeg|gif|webp|avif|svg\+xml)|font\/(woff2?|ttf|otf)|application\/font-woff)[;,]/.test(url) ? intern(url) : "";
    if (this.assets.has(url)) return this.assets.get(url);
    const response = await this.page.context().request.get(url, { timeout: 3000, maxRedirects: 5 });
    try {
      const type = (response.headers()["content-type"] || "").split(";")[0];
      if (!/^(image\/(png|jpeg|gif|webp|avif|svg\+xml)|font\/(woff2?|ttf|otf)|application\/font-woff)$/.test(type)) return "";
      const body = await response.body();
      if (body.length > MAX_BYTES || this.assets.size >= 1024) throw Error("Asset limit");
      const data = `data:${type};base64,${body.toString("base64")}`;
      const reference = intern(data);
      this.assets.set(url, reference);
      return reference;
    } finally { await response.dispose(); }
  }
  async waitForChange(version) {
    if (this.disposed) throw Error("Document detached");
    await this.frame.evaluate(({ key, version }) => {
      const state = globalThis[key];
      if (!state || state.version !== version) return;
      const delay = Math.max(0, Math.min(state.hasFrames ? 100 : 800, 2000 - (performance.now() - state.fullAt)));
      if (!delay) return;
      return new Promise(resolve => {
        const done = () => { clearTimeout(timer); state.waiters.delete(done); resolve(); };
        const timer = setTimeout(done, delay); state.waiters.add(done);
      });
    }, { key: this.key, version });
    if (this.disposed) throw Error("Document detached");
  }
  capture(budget = { frames: 16 }) {
    if (this.disposed) return Promise.reject(Error("Document detached"));
    if (!this.pendingCapture) this.pendingCapture = this.captureCurrent(budget).then(snapshot => {
      this.captureFailed = false; return snapshot;
    }, error => { this.captureFailed = true; throw error; }).finally(() => { this.pendingCapture = null; });
    return this.pendingCapture;
  }
  async captureCurrent(budget) {
    const captureStarted = performance.now();
    if (--budget.frames < 0) throw Error("Frame limit");
    const generation = this.generation;
    let result;
    try {
      const established = this.lastCapture?.generation === generation;
      result = JSON.parse(await this.frame.evaluate(observeDocument, { key: this.key, generation, established, reuse: !this.captureFailed && established, knownStyles: [...this.sourceStyles].map(([id, style]) => [id, style.version]) }));
    } catch (error) {
      if (/^frame\.evaluate: Error: (Source identity limit|Document scan limit|Visible document limit)\b/.test(error.message)) error.code = 'DOCUMENT_LIMIT';
      throw error;
    }
    if (this.disposed) throw Error("Document detached");
    if (result.replaced) {
      if (generation === this.generation) this.replaceDocument();
      budget.frames++;
      return this.captureCurrent(budget);
    }
    if (generation !== this.generation) throw Error("Navigation during capture");
    if (result.unchanged && this.lastCapture?.generation === generation) {
      this.metrics = { ...this.metrics, captureMs: performance.now() - captureStarted, inspectedNodes: result.inspectedNodes, sourceStyleBytes: 0, cached: true };
      return this.lastCapture = { ...this.lastCapture, sourceVersion: result.sourceVersion, revision: ++this.revision };
    }
    const sourceStyleBytes = Buffer.byteLength(JSON.stringify(result.styles));
    const pending = new Map();
    const asset = url => {
      if ((this.failedAssets.get(url) || 0) > Date.now()) return Promise.resolve("");
      if (!pending.has(url)) pending.set(url, this.fetchAsset(async () => {
        try { return await this.asset(url, generation); }
        catch {
          if (generation !== this.generation) return "";
          if (this.failedAssets.size >= 128) this.failedAssets.delete(this.failedAssets.keys().next().value);
          this.failedAssets.set(url, Date.now() + 30000);
          return "";
        }
      }));
      return pending.get(url);
    };
    const rewrite = async (text, base) => {
      const key = createHash('sha256').update(base + text).digest('hex');
      const cached = this.rewrites.get(key);
      if (cached && cached.expires > Date.now()) return cached.text;
      const matches = [...text.matchAll(/url\(\s*(['"]?)(.*?)\1\s*\)/gi)];
      const replacements = new Map();
      const queue = [...new Set(matches.map(match => match[2]).filter(url => !url.startsWith('#')))];
      await Promise.all(Array.from({ length: Math.min(4, queue.length) }, async () => {
        while (queue.length) { const url = queue.shift(); replacements.set(url, await asset(new URL(url, base).href)); }
      }));
      text = text.replace(/url\(\s*(['"]?)(.*?)\1\s*\)/gi, (match, quote, url) => replacements.has(url) ? `url("${replacements.get(url)}")` : match);
      if ([...this.rewrites.values()].reduce((n, v) => n + v.text.length, text.length) > MAX_BYTES) this.rewrites.clear();
      if (this.disposed) throw Error("Document detached");
      if (generation !== this.generation) throw Error("Navigation during capture");
      this.rewrites.set(key, { text, expires: Date.now() + 30000 });
      return text;
    };
    const jobs = [];
    const liveFrames = new Set();
    const visit = async (node) => {
      if (node.tag === "iframe" && budget.frames > 0) {
        const handle = await this.frame.evaluateHandle(({ key, id }) => globalThis[key]?.reverse.get(id), { key: this.key, id: node.id });
        try {
          const frame = await handle.asElement()?.contentFrame();
          if (frame && !frame.isDetached()) {
            let child = this.frames.get(node.id);
            if (child?.frame !== frame) { child?.dispose(); child = new BrowserDocument(frame, this.preserveWindow); this.frames.set(node.id, child); }
            liveFrames.add(node.id);
            node.document = await child.capture(budget);
            result.unsupported = result.unsupported.filter(region => region.id !== node.id);
          }
        } catch { /* An inaccessible frame keeps its explicit unsupported region. */ }
        finally { await handle.dispose(); }
      }
      if (node.tag === "img") jobs.push(async () => { node.attrs.src = node.attrs.src ? await asset(new URL(node.attrs.src, result.base).href) : ""; });
      if (node.tag === "video" && node.attrs.poster) jobs.push(async () => { node.attrs.poster = await asset(new URL(node.attrs.poster, result.base).href); });
      if (node.attrs?.style) jobs.push(async () => { node.attrs.style = await rewrite(node.attrs.style, result.base); });
      for (const child of node.children || []) await visit(child);
      for (const child of node.shadow || []) await visit(child);
      if (node.tag === "style") for (const child of node.children || []) if (child.text) jobs.push(async () => { child.text = await rewrite(child.text, result.base); });
      if (node.shadowStyles) jobs.push(async () => { node.shadowStyles = await Promise.all(node.shadowStyles.map(text => rewrite(text, result.base))); });
    };
    await visit(result.root);
    const sourceStyles = new Map();
    await concurrent(result.styles, 8, async (style, index) => {
      if (style.text === undefined && !style.url) {
        const cached = this.sourceStyles.get(style.id);
        if (!cached || cached.version !== style.version) throw Error("Stylesheet checkpoint required");
        style = { ...cached };
      }
      if (style.url) {
        const response = await this.page.context().request.get(style.url, { timeout: 3000 });
        try { style.text = await response.text(); } finally { await response.dispose(); }
        style.base = style.url;
        delete style.url;
      }
      if (style.text.length > MAX_BYTES) throw Error("Stylesheet limit");
      sourceStyles.set(style.id, { ...style });
      jobs.push(async () => { result.styles[index] = { text: await rewrite(style.text, style.base) }; });
    });
    await concurrent(jobs, 8, job => job());
    for (const [id, child] of this.frames) if (!liveFrames.has(id)) { child.dispose(); this.frames.delete(id); }
    if (this.disposed) throw Error("Document detached");
    if (generation !== this.generation) throw Error("Navigation during capture");
    this.sourceStyles = sourceStyles;
    const styleResources = {};
    result.styles = result.styles.map(style => {
      const key = createHash('sha256').update(style.text).digest('hex');
      styleResources[key] = style.text;
      return { resource: key };
    });
    this.styles = new Map(Object.entries(styleResources));
    const snapshot = { ...result, styleResources, resources: Object.fromEntries(this.resources), generation, revision: ++this.revision };
    this.metrics = { captureMs: performance.now() - captureStarted, inspectedNodes: result.inspectedNodes, sourceStyleBytes, documentBytes: Buffer.byteLength(JSON.stringify(result.root)), styleBytes: Buffer.byteLength(JSON.stringify(styleResources)), resourceBytes: Buffer.byteLength(JSON.stringify(snapshot.resources)) };
    if (Buffer.byteLength(JSON.stringify({ ...snapshot, resources: {}, styleResources: {} })) > MAX_BYTES || this.metrics.styleBytes > MAX_BYTES) throw Error("Snapshot limit");
    this.lastCapture = snapshot;
    return snapshot;
  }
}
