import { randomBytes, createHash } from "node:crypto";
import { DocumentMedia } from "./document-media.mjs";

const MAX_BYTES = 8 * 1024 * 1024;

// Runs in the source document. IDs survive observations but never navigation.
function observeDocument(key) {
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
    const result = { id: id(node), tag, ns: node.namespaceURI, attrs, children: [] };
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement || node instanceof HTMLSelectElement) {
      result.value = node.type === "password" ? "" : node.value;
      result.checked = !!node.checked;
      if (node.type === "password") delete attrs.value;
    }
    if (node.isContentEditable && !node.parentElement?.isContentEditable) result.value = node.textContent;
    if (["canvas", "video", "audio", "iframe", "object", "embed"].includes(tag)) {
      if (typeof node.captureStream === "function") result.media = true;
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
  for (const sheet of document.styleSheets) {
    try { styles.push({ text: [...sheet.cssRules].map(r => r.cssText).join("\n"), base: sheet.href || document.baseURI }); }
    catch { styles.push({ url: sheet.href }); }
  }
  return { root: visit(document.documentElement), styles, unsupported, base: document.baseURI, width: innerWidth, height: innerHeight };
}

export class BrowserDocument {
  constructor(page) {
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
    this.navigation = (frame) => {
      if (frame === this.frame) {
        void this.media.dispose();
        this.generation = randomBytes(16).toString("hex");
        this.assets.clear();
        this.resources.clear();
        this.styles.clear();
      }
    };
    this.page.on("framenavigated", this.navigation);
  }
  dispose() {
    void this.media.dispose();
    this.page.off("framenavigated", this.navigation);
    this.assets.clear();
    this.resources.clear();
    this.styles.clear();
    for (const child of this.frames.values()) child.dispose();
    this.frames.clear();
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
      if (command.frames.length > 16 || route.generation !== this.generation || !this.frames.has(route.node)) throw Error("Stale frame route");
      return this.frames.get(route.node).dispatch({ ...command, frames });
    }
    if (command.generation !== this.generation || !Number.isSafeInteger(command.revision) || command.revision < 1 || command.revision > this.revision) throw Error("Stale document");
    if (command.action === "resize") {
      const { width, height } = command;
      if (![width, height].every(n => Number.isInteger(n) && n >= 240 && n <= 2560)) throw Error("Viewport limit");
      await this.page.setViewportSize({ width, height });
      this.viewportChanged = true;
      return;
    }
    if (!Number.isSafeInteger(command.node)) throw Error("Invalid document target");
    const handle = await this.frame.evaluateHandle(({ key, id }) => globalThis[key]?.reverse.get(id), { key: this.key, id: command.node });
    try {
      const element = handle.asElement();
      if (!element || !await element.evaluate(n => n.isConnected && !n.closest('[inert]') && !n.disabled)) throw Error("Unavailable document target");
      if (command.action === "click") {
        let position;
        if (command.position) {
          if (![command.position.x, command.position.y].every(n => Number.isFinite(n) && n >= 0 && n <= 1)) throw Error("Pointer position rejected");
          const box = await element.boundingBox();
          if (!box) throw Error("Invisible target");
          position = { x: command.position.x * box.width, y: command.position.y * box.height };
        }
        await element.click({ timeout: 1000, position });
      }
      else if (command.action === "value") {
        if (typeof command.value !== "string" || command.value.length > 16384) throw Error("Text limit");
        const valid = await element.evaluate((n, expected) => n.type !== 'password' && String(n.value ?? '') === expected, command.expected);
        if (!valid) throw Error("Source text changed");
        if (await element.evaluate(n => n.localName === 'select')) await element.selectOption(command.value, { timeout: 1000 });
        else await element.fill(command.value, { timeout: 1000 });
      } else if (command.action === "rich-value") {
        if (typeof command.value !== 'string' || command.value.length > 16384 || typeof command.expected !== 'string') throw Error("Text limit");
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
        if (!edit) throw Error("Source text changed");
        if (edit.noop) return;
        if (edit.text) await this.page.keyboard.insertText(edit.text);
        else await this.page.keyboard.press('Backspace');
      } else if (command.action === "scroll") {
        if (![command.x, command.y].every(n => Number.isFinite(n) && Math.abs(n) <= 10000000)) throw Error("Scroll limit");
        await element.evaluate((n, c) => n.scrollTo(c.x, c.y), command);
      } else throw Error("Unsupported document action");
    } finally { await handle.dispose(); }
  }
  async asset(url) {
    const intern = data => {
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
      if (body.length > MAX_BYTES || this.assets.size >= 128) throw Error("Asset limit");
      const data = `data:${type};base64,${body.toString("base64")}`;
      const reference = intern(data);
      this.assets.set(url, reference);
      return reference;
    } finally { await response.dispose(); }
  }
  async capture(budget = { frames: 16 }) {
    if (--budget.frames < 0) throw Error("Frame limit");
    const generation = this.generation;
    const result = await this.frame.evaluate(observeDocument, this.key);
    const rewrite = async (text, base) => {
      const matches = [...text.matchAll(/url\(\s*(['"]?)(.*?)\1\s*\)/gi)];
      for (const match of matches) {
        if (match[2].startsWith("#")) continue;
        const resource = await this.asset(new URL(match[2], base).href).catch(() => "");
        text = text.replace(match[0], `url("${resource}")`);
      }
      return text;
    };
    for (const style of result.styles) {
      if (style.url) {
        const response = await this.page.context().request.get(style.url, { timeout: 3000 });
        try { style.text = await response.text(); } finally { await response.dispose(); }
        style.base = style.url;
      }
      if (style.text.length > MAX_BYTES) throw Error("Stylesheet limit");
      style.text = await rewrite(style.text, style.base);
    }
    const liveFrames = new Set();
    const visit = async (node) => {
      if (node.tag === "iframe" && budget.frames > 0) {
        const handle = await this.frame.evaluateHandle(({ key, id }) => globalThis[key]?.reverse.get(id), { key: this.key, id: node.id });
        try {
          const frame = await handle.asElement()?.contentFrame();
          if (frame && !frame.isDetached()) {
            let child = this.frames.get(node.id);
            if (child?.frame !== frame) { child?.dispose(); child = new BrowserDocument(frame); this.frames.set(node.id, child); }
            liveFrames.add(node.id);
            node.document = await child.capture(budget);
            result.unsupported = result.unsupported.filter(region => region.id !== node.id);
          }
        } catch { /* An inaccessible frame keeps its explicit unsupported region. */ }
        finally { await handle.dispose(); }
      }
      if (node.tag === "img") node.attrs.src = await this.asset(new URL(node.attrs.src, result.base).href).catch(() => "");
      if (node.attrs?.style) node.attrs.style = await rewrite(node.attrs.style, result.base);
      for (const child of node.children || []) await visit(child);
      for (const child of node.shadow || []) await visit(child);
      if (node.tag === "style") for (const child of node.children || []) if (child.text) child.text = await rewrite(child.text, result.base);
      if (node.shadowStyles) node.shadowStyles = await Promise.all(node.shadowStyles.map(text => rewrite(text, result.base)));
    };
    await visit(result.root);
    for (const [id, child] of this.frames) if (!liveFrames.has(id)) { child.dispose(); this.frames.delete(id); }
    if (generation !== this.generation) throw Error("Navigation during capture");
    const styleResources = {};
    result.styles = result.styles.map(style => {
      const key = createHash('sha256').update(style.text).digest('hex');
      styleResources[key] = style.text;
      return { resource: key };
    });
    this.styles = new Map(Object.entries(styleResources));
    const snapshot = { ...result, styleResources, resources: Object.fromEntries(this.resources), generation, revision: ++this.revision };
    this.metrics = { documentBytes: Buffer.byteLength(JSON.stringify(result.root)), styleBytes: Buffer.byteLength(JSON.stringify(styleResources)), resourceBytes: Buffer.byteLength(JSON.stringify(snapshot.resources)) };
    if (Buffer.byteLength(JSON.stringify({ ...snapshot, resources: {}, styleResources: {} })) > MAX_BYTES || this.metrics.styleBytes > MAX_BYTES) throw Error("Snapshot limit");
    return snapshot;
  }
}
