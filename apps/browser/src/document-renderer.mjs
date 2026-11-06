const HTML = "http://www.w3.org/1999/xhtml";
const SVG = "http://www.w3.org/2000/svg";
const BLOCKED = new Set(["script", "noscript", "iframe", "object", "embed", "base", "meta", "link", "style", "foreignObject", "animate", "set"]);
const BLOCKED_ATTR = /^(on.*|src|srcset|srcdoc|poster|href|xlink:href|action|formaction|ping|target|formtarget|nonce|integrity|autofocus|autoplay)$/i;

function attribute(element, name, value) {
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}

export class DocumentRenderer {
  constructor(iframe, dispatch = () => {}, canControl = () => false, media, frames = []) {
    this.iframe = iframe;
    // Native canvas and media need scripting enabled; CSP forbids all page scripts.
    iframe.setAttribute("sandbox", "allow-same-origin allow-scripts");
    this.generation = null;
    this.dispatch = dispatch;
    this.canControl = canControl;
    this.media = media;
    this.route = frames;
    this.drafts = new Map();
    this.serial = 0;
    this.localScrolls = new Map();
    this.remoteScrolls = new WeakMap();
  }
  cancel() { this.serial++; this.cancelLoad?.(); for (const child of this.frames?.values() || []) child.cancel(); }
  async render(snapshot, signal) {
    this.cancelLoad?.();
    const serial = ++this.serial;
    const check = () => {
      signal?.throwIfAborted();
      if (serial !== this.serial) throw new DOMException('Document render cancelled', 'AbortError');
    };
    check();
    if (!snapshot.root || JSON.stringify(snapshot).length > 24 * 1024 * 1024) throw Error("Document and resource limit");
    const iframe = this.iframe;
    if (snapshot.generation !== this.generation) await new Promise((resolve, reject) => {
      const done = error => {
        clearTimeout(timer);
        iframe.removeEventListener('load', loaded);
        signal?.removeEventListener('abort', cancelled);
        if (this.cancelLoad === cancelled) this.cancelLoad = null;
        if (error) reject(error); else resolve();
      };
      const loaded = () => {
        if (iframe.contentDocument?.querySelector('meta[name="rui-load"]')?.content === String(serial)) done();
      };
      const cancelled = () => done(new DOMException('Document render cancelled', 'AbortError'));
      const timer = setTimeout(() => done(Error('Document frame load timed out')), 5000);
      this.cancelLoad = cancelled;
      iframe.addEventListener('load', loaded);
      signal?.addEventListener('abort', cancelled, { once: true });
      iframe.srcdoc = `<!doctype html><html><head><meta name="rui-load" content="${serial}"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src blob: mediastream:; form-action 'none'; base-uri 'none'"></head><body></body></html>`;
    });
    check();
    const doc = iframe.contentDocument;
    if (snapshot.generation !== this.generation) {
      this.managed = new WeakSet();
      this.styleElements = new Map();
      this.localScrolls.clear();
      this.remoteScrolls = new WeakMap();
    }
    const nextStyles = new Map();
    const resource = reference => snapshot.resources?.[reference?.replace(/^rui-resource:/, "")] || "";
    const css = text => text.replace(/rui-resource:([a-f0-9]{64})/g, (_, key) => snapshot.resources?.[key] || "");
    if (snapshot.generation !== this.generation) {
      const send = (node, action) => {
        if (!this.canControl() || this.generation !== snapshot.generation || iframe.contentDocument !== doc) return;
        this.dispatch({ generation: this.generation, revision: this.revision, node, ...action });
      };
      let gesture, suppressClick = 0;
      const point = (target, event) => {
        const rect = target.getBoundingClientRect();
        return { x: Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (event.clientY - rect.top) / rect.height)) };
      };
      doc.addEventListener("pointerdown", event => {
        const target = event.composedPath().find(node => this.ids?.has(node) && node.matches?.('canvas,[draggable=true],[role=slider]'));
        if (target && this.canControl() && event.isPrimary) {
          event.preventDefault(); target.setPointerCapture(event.pointerId);
          gesture = { target, pointer: event.pointerId, x: event.clientX, y: event.clientY, points: [point(target, event)] };
        }
      }, true);
      doc.addEventListener("pointermove", event => {
        if (!gesture || event.pointerId !== gesture.pointer) return;
        if (Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) < 4) return;
        gesture.moved = true;
        if (gesture.points.length < 63) gesture.points.push(point(gesture.target, event));
      }, true);
      doc.addEventListener("pointerup", event => {
        if (!gesture || event.pointerId !== gesture.pointer) return;
        const current = gesture; gesture = null;
        if (!current.moved) return;
        suppressClick = Date.now() + 500;
        send(this.ids.get(current.target), { action: "drag", points: [...current.points, point(current.target, event)] });
      }, true);
      doc.addEventListener("pointercancel", () => { gesture = null; }, true);
      doc.addEventListener("keydown", event => {
        const target = event.composedPath().find(node => this.ids?.has(node));
        if (event.key !== 'Enter' || event.isComposing || target?.localName !== 'input' || ['password', 'checkbox', 'radio', 'button', 'submit', 'file'].includes(target.type)) return;
        event.preventDefault();
        const node = this.ids.get(target);
        send(node, { action: 'value', value: target.value, expected: this.drafts.has(node) ? this.drafts.get(node) : this.values.get(node), key: 'Enter' });
        this.drafts.delete(node);
        this.values.set(node, target.value);
      }, true);
      doc.addEventListener("click", event => {
        event.preventDefault();
        if (event.composedPath().some(node => node.hasAttribute?.('data-rui-deferred'))) return;
        if (Date.now() < suppressClick) return;
        const path = event.composedPath().filter(node => this.ids?.has(node));
        const control = path.find(node => node.matches?.("button,a[href],input,select,textarea,[role=button],[role=slider],[role=checkbox],[role=link]"));
        if (!control && this.media?.activate?.(event)) return;
        const target = control || path[0];
        if (target) {
          const rect = target.getBoundingClientRect();
          const position = event.detail && rect.width && rect.height ? { x: Math.max(0, Math.min(1, (event.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (event.clientY - rect.top) / rect.height)) } : undefined;
          send(this.ids.get(target), { action: "click", position });
        }
      }, true);
      doc.addEventListener("beforeinput", event => {
        if (!this.canControl()) { event.preventDefault(); return; }
        const id = this.ids?.get(event.target);
        if (id && !this.drafts.has(id)) this.drafts.set(id, this.values.get(id));
      }, true);
      doc.addEventListener("change", event => {
        const node = this.ids?.get(event.target);
        if (node && typeof event.target.value === "string" && (this.drafts.has(node) || event.target.value !== this.values.get(node))) { send(node, { action: "value", value: event.target.value, expected: this.drafts.has(node) ? this.drafts.get(node) : this.values.get(node) }); this.drafts.delete(node); }
      }, true);
      doc.addEventListener("focusout", event => {
        const node = this.ids?.get(event.target);
        if (node && event.target.isContentEditable && this.drafts.has(node)) {
          send(node, { action: "rich-value", value: event.target.textContent, expected: this.drafts.get(node) });
          this.drafts.delete(node);
        }
      }, true);
      const scrollTimers = new Map();
      doc.addEventListener("scroll", event => {
        if (iframe.contentDocument !== doc) return;
        const target = event.target === doc ? doc.scrollingElement : event.target;
        const node = this.ids?.get(target);
        if (!node) return;
        const position = { x: target.scrollLeft, y: target.scrollTop };
        const remote = this.remoteScrolls.get(target);
        this.remoteScrolls.delete(target);
        if (remote && Math.abs(remote.x - position.x) < 1 && Math.abs(remote.y - position.y) < 1) return;
        this.localScrolls.set(node, { ...position, time: Date.now() });
        clearTimeout(scrollTimers.get(node));
        scrollTimers.set(node, setTimeout(() => {
          scrollTimers.delete(node);
          if (target.isConnected) send(node, { action: 'scroll', x: target.scrollLeft, y: target.scrollTop });
        }, 100));
      }, true);
    }
    const reconcile = (parent, children) => {
      let cursor = parent.firstChild;
      for (const child of children) {
        while (cursor && this.managed.has(cursor)) cursor = cursor.nextSibling;
        if (cursor !== child) parent.insertBefore(child, cursor);
        cursor = child.nextSibling;
      }
      while (cursor) {
        const next = cursor.nextSibling;
        if (!this.managed.has(cursor)) cursor.remove();
        cursor = next;
      }
    };
    const styles = (parent, texts) => {
      const previous = this.styleElements.get(parent) || [];
      const entries = texts.map((text, index) => {
        const entry = previous[index] || { element: doc.createElement("style") };
        this.managed.add(entry.element);
        if (entry.text !== text) { entry.element.textContent = css(text); entry.text = text; }
        return entry;
      });
      for (const entry of previous.slice(entries.length)) entry.element.remove();
      let cursor = null;
      for (const entry of entries.toReversed()) {
        if (entry.element.parentNode !== parent || entry.element.nextSibling !== cursor) parent.insertBefore(entry.element, cursor);
        cursor = entry.element;
      }
      nextStyles.set(parent, entries);
    };
    const previous = snapshot.generation === this.generation ? this.nodes : new Map();
    if (snapshot.generation !== this.generation) this.frames = new Map();
    const pendingFrames = [];
    const pendingMedia = [];
    const pendingScrolls = [];
    this.nodes = new Map();
    this.ids = new WeakMap();
    this.values = new Map();
    let count = 0;
    const build = (node, depth = 0) => {
      if (++count > 20000 || depth > 128 || this.nodes.has(node.id)) throw Error("Document structure limit");
      if (typeof node.text === "string") {
        const old = previous.get(node.id);
        const text = old?.nodeType === 3 ? old : doc.createTextNode(node.text);
        const value = node.text.includes('rui-resource:') ? css(node.text) : node.text;
        if (text.data !== value) text.data = value;
        this.nodes.set(node.id, text);
        return text;
      }
      if (![HTML, SVG].includes(node.ns) || !/^[a-zA-Z][\w-]*$/.test(node.tag)) throw Error("Unsupported element");
      const embedded = node.tag === "iframe" && node.document;
      const blocked = BLOCKED.has(node.tag) && node.tag !== "style" && !embedded;
      const tag = blocked ? "span" : node.tag;
      const ns = blocked ? HTML : node.ns;
      const old = previous.get(node.id);
      const element = old?.localName === tag && old?.namespaceURI === ns ? old : doc.createElementNS(ns, tag);
      const keep = new Set();
      if (embedded) keep.add('sandbox');
      if (node.tag === 'video') keep.add('poster');
      if (node.media) { keep.add('autoplay'); keep.add('playsinline'); keep.add('preload'); }
      this.nodes.set(node.id, element);
      this.ids.set(element, node.id);
      if (typeof node.value === "string") this.values.set(node.id, node.value);
      for (const [name, value] of Object.entries(node.attrs || {})) {
        if (embedded && name === 'sandbox') continue;
        if (node.media && name === 'preload') continue;
        if (!BLOCKED_ATTR.test(name) && typeof value === "string") {
          keep.add(name);
          const actual = name === 'style' ? css(value) : value;
          if (element.getAttribute(name) !== actual) element.setAttribute(name, actual);
        }
      }
      for (const name of ['href', 'xlink:href']) {
        const value = node.attrs?.[name];
        if (typeof value === 'string' && (node.tag === 'a' || value.startsWith('#'))) {
          keep.add(name);
          attribute(element, name, value.startsWith('#') ? value : '#');
        }
      }
      for (const attr of [...element.attributes]) if (!keep.has(attr.name) && attr.name !== "src") element.removeAttribute(attr.name);
      if (node.deferred) {
        element.setAttribute('data-rui-deferred', '');
        if (node.deferred.hidden) element.style.setProperty('display', 'none', 'important');
        else {
          for (const dimension of ['width', 'height']) {
            const value = node.deferred[dimension];
            if (typeof value !== 'string' || !/^\d+(?:\.\d+)?px$/.test(value)) throw Error('Invalid deferred region size');
            for (const prefix of ['', 'min-', 'max-']) element.style.setProperty(prefix + dimension, value, 'important');
          }
          element.style.setProperty('overflow', 'hidden', 'important');
        }
      }
      if (node.scroll) {
        const { x, y } = node.scroll;
        if (![x, y].every(value => Number.isFinite(value) && Math.abs(value) <= 10000000)) throw Error('Invalid document scroll');
        pendingScrolls.push(() => {
          const local = this.localScrolls.get(node.id);
          if (local && (Math.abs(local.x - x) >= 1 || Math.abs(local.y - y) >= 1) && Date.now() - local.time < 5000) return;
          this.localScrolls.delete(node.id);
          if (Math.abs(element.scrollLeft - x) < 1 && Math.abs(element.scrollTop - y) < 1) return;
          this.remoteScrolls.set(element, { x, y });
          element.scrollTo({ left: x, top: y, behavior: 'instant' });
        });
      }
      if (node.tag === 'canvas' || node.attrs?.draggable === 'true' || node.attrs?.role === 'slider') element.style.touchAction = 'none';
      const image = resource(node.attrs?.src);
      if (node.tag === "img") {
        if (/^data:image\/(png|jpeg|gif|webp|avif|svg\+xml)[;,]/.test(image)) attribute(element, "src", image);
        else element.removeAttribute('src');
      }
      if (node.tag === 'video') {
        const poster = resource(node.attrs?.poster);
        if (/^data:image\/(png|jpeg|gif|webp|avif|svg\+xml)[;,]/.test(poster)) attribute(element, 'poster', poster);
        else element.removeAttribute('poster');
      }
      if (embedded) {
        attribute(element, "sandbox", "allow-same-origin allow-scripts");
        let renderer = this.frames.get(node.id);
        if (!renderer || renderer.iframe !== element) {
          renderer = new DocumentRenderer(element, command => this.dispatch({ ...command, frames: [{ node: node.id, generation: this.generation }, ...(command.frames || [])] }), this.canControl, this.media, [...this.route, { node: node.id, generation: snapshot.generation }]);
          this.frames.set(node.id, renderer);
        }
        pendingFrames.push(() => renderer.render(node.document, signal));
      } else if (node.media && this.media) {
        pendingMedia.push(() => this.media.attach(node, element, snapshot.generation, this.route));
      } else if (blocked || ["canvas", "video", "audio"].includes(node.tag)) {
        element.textContent = `Unsupported region: ${node.tag}`;
        element.setAttribute("data-rui-unsupported", node.tag);
      } else if (!(element === doc.activeElement && element.isContentEditable && this.drafts.has(node.id))) {
        const children = (node.children || []).map(child => build(child, depth + 1));
        reconcile(element, children);
      }
      if (typeof node.value === "string" && "value" in element && element !== doc.activeElement && element.value !== node.value) element.value = node.value;
      if (node.tag === 'input' && node.attrs?.type === 'password') {
        element.readOnly = true;
        element.placeholder = 'Use the password control below the page';
      }
      if (typeof node.checked === "boolean" && "checked" in element) element.checked = node.checked;
      if (node.shadow) {
        const shadow = element.shadowRoot || element.attachShadow({ mode: "open" });
        const children = node.shadow.map(child => build(child, depth + 1));
        reconcile(shadow, children);
        styles(shadow, node.shadowStyles || []);
      }
      return element;
    };
    const csp = doc.head.querySelector("meta[http-equiv]");
    if (csp) this.managed.add(csp);
    const root = build(snapshot.root);
    const head = root.querySelector("head") || root.insertBefore(doc.createElement("head"), root.firstChild);
    if (csp && head.firstChild !== csp) head.prepend(csp);
    styles(head, snapshot.styles.map(sheet => sheet.text ?? snapshot.styleResources?.[sheet.resource] ?? ""));
    this.styleElements = nextStyles;
    if (root !== doc.documentElement) doc.replaceChild(root, doc.documentElement);
    this.generation = snapshot.generation;
    this.revision = snapshot.revision;
    for (const render of pendingFrames) { await render(); check(); }
    for (const attach of pendingMedia) attach();
    for (const scroll of pendingScrolls) scroll();
    for (const id of this.frames.keys()) if (!this.nodes.has(id)) this.frames.delete(id);
  }
}
