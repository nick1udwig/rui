import { randomBytes, createHash } from "node:crypto";
import { DocumentMedia } from "./document-media.mjs";
import { concurrent, taskPool } from "./src/document-work.mjs";

const MAX_BYTES = 8 * 1024 * 1024;

// Runs in the source document. IDs survive observations but never navigation.
function observeDocument({ key, knownStyles }) {
  const state = globalThis[key] ||= { nodes: new WeakMap(), reverse: new Map(), next: 1 };
  const id = (node) => {
    if (!state.nodes.has(node)) {
      if (state.next > 100000) throw Error("Source identity limit");
      state.nodes.set(node, state.next++);
    }
    state.reverse.set(state.nodes.get(node), node);
    return state.nodes.get(node);
  };
  const unsupported = [];
  state.reverse.clear();
  let count = 0;
  const visit = (node, depth = 0, shadow = false) => {
    if (++count > 20000 || depth > 128) throw Error("Document limit");
    if (node.nodeType === 3) return { id: id(node), text: node.data };
    if (node.nodeType !== 1) return null;
    const tag = node.localName;
    if (["script", "noscript", "base", "meta", "link"].includes(tag) || (tag === "style" && !shadow)) return null;
    const attrs = Object.fromEntries([...node.attributes].filter(a => !/^on/i.test(a.name) && !["srcdoc", "action", "formaction", "nonce"].includes(a.name)).map(a => [a.name, a.value]));
    if (tag === "img") attrs.src = node.currentSrc || node.src;
    if (tag === "video" && node.poster) attrs.poster = node.poster;
    const result = { id: id(node), tag, ns: node.namespaceURI, attrs, children: [] };
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement || node instanceof HTMLSelectElement) {
      result.value = node.type === "password" ? "" : node.value;
      result.checked = !!node.checked;
      if (node.type === "password") delete attrs.value;
    }
    if (node.isContentEditable && !node.parentElement?.isContentEditable) result.value = node.textContent;
    if (["canvas", "video", "audio", "iframe", "object", "embed"].includes(tag)) {
      if (typeof node.captureStream === "function") { result.media = true; result.mediaReady = tag === "canvas" || node.readyState >= 2; }
      else unsupported.push({ id: result.id, kind: tag });
      return result;
    }
    result.children = [...node.childNodes].map(n => visit(n, depth + 1, shadow)).filter(Boolean);
    if (node.shadowRoot) {
      result.shadow = [...node.shadowRoot.childNodes].map(n => visit(n, depth + 1, true)).filter(Boolean);
      result.shadowStyles = [...node.shadowRoot.adoptedStyleSheets].map(sheet => [...sheet.cssRules].map(rule => rule.cssText).join("\n"));
    }
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
  // Avoid Playwright's per-property encoding for this large, JSON-only snapshot.
  return JSON.stringify({ root: visit(document.documentElement), styles, unsupported, base: document.baseURI, width: innerWidth, height: innerHeight });
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
    this.navigation = (frame) => {
      if (frame === this.frame) {
        void this.media.dispose();
        this.generation = randomBytes(16).toString("hex");
        this.assets.clear();
        this.resources.clear();
        this.styles.clear();
        this.rewrites.clear();
        this.sourceStyles.clear();
        this.failedAssets.clear();
      }
    };
    this.page.on("framenavigated", this.navigation);
  }
  dispose() {
    void this.restoreViewport();
    void this.media.dispose();
    this.page.off("framenavigated", this.navigation);
    this.assets.clear();
    this.resources.clear();
    this.styles.clear();
    this.rewrites.clear();
    this.sourceStyles.clear();
    this.failedAssets.clear();
    for (const child of this.frames.values()) child.dispose();
    this.frames.clear();
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
    const handle = await this.frame.evaluateHandle(({ key, id }) => globalThis[key]?.reverse.get(id), { key: this.key, id: command.node });
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
  async capture(budget = { frames: 16 }) {
    if (--budget.frames < 0) throw Error("Frame limit");
    const generation = this.generation;
    const result = JSON.parse(await this.frame.evaluate(observeDocument, { key: this.key, knownStyles: [...this.sourceStyles].map(([id, style]) => [id, style.version]) }));
    if (generation !== this.generation) throw Error("Navigation during capture");
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
    this.metrics = { sourceStyleBytes, documentBytes: Buffer.byteLength(JSON.stringify(result.root)), styleBytes: Buffer.byteLength(JSON.stringify(styleResources)), resourceBytes: Buffer.byteLength(JSON.stringify(snapshot.resources)) };
    if (Buffer.byteLength(JSON.stringify({ ...snapshot, resources: {}, styleResources: {} })) > MAX_BYTES || this.metrics.styleBytes > MAX_BYTES) throw Error("Snapshot limit");
    return snapshot;
  }
}
