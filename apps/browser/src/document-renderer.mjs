const HTML = "http://www.w3.org/1999/xhtml";
const SVG = "http://www.w3.org/2000/svg";
const BLOCKED = new Set(["script", "noscript", "iframe", "object", "embed", "base", "meta", "link", "style", "foreignObject", "animate", "set"]);
const BLOCKED_ATTR = /^(on.*|src|srcset|srcdoc|href|xlink:href|action|formaction|ping|target|formtarget|nonce|integrity|autofocus|autoplay)$/i;

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
  }
  async render(snapshot) {
    if (!snapshot.root || JSON.stringify(snapshot).length > 24 * 1024 * 1024) throw Error("Document and resource limit");
    const iframe = this.iframe;
    if (snapshot.generation !== this.generation) await new Promise(resolve => {
      iframe.onload = resolve;
      iframe.srcdoc = `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'"></head><body></body></html>`;
    });
    const doc = iframe.contentDocument;
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
        if (Date.now() < suppressClick) return;
        const path = event.composedPath().filter(node => this.ids?.has(node));
        const target = path.find(node => node.matches?.("button,a[href],input,select,textarea,[role=button],[role=slider],[role=checkbox],[role=link]")) || path[0];
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
      let scrollTimer;
      doc.addEventListener("scroll", event => {
        const target = event.target === doc ? doc.scrollingElement : event.target;
        const node = this.ids?.get(target);
        clearTimeout(scrollTimer);
        scrollTimer = setTimeout(() => { if (node && target.isConnected) send(node, { action: "scroll", x: target.scrollLeft, y: target.scrollTop }); }, 100);
      }, true);
    }
    const previous = snapshot.generation === this.generation ? this.nodes : new Map();
    if (snapshot.generation !== this.generation) this.frames = new Map();
    const pendingFrames = [];
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
      this.nodes.set(node.id, element);
      this.ids.set(element, node.id);
      if (typeof node.value === "string") this.values.set(node.id, node.value);
      for (const [name, value] of Object.entries(node.attrs || {})) {
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
          element.setAttribute(name, value.startsWith('#') ? value : '#');
        }
      }
      for (const attr of [...element.attributes]) if (!keep.has(attr.name) && attr.name !== "src") element.removeAttribute(attr.name);
      if (node.tag === 'canvas' || node.attrs?.draggable === 'true' || node.attrs?.role === 'slider') element.style.touchAction = 'none';
      const image = resource(node.attrs?.src);
      if (node.tag === "img") {
        if (/^data:image\/(png|jpeg|gif|webp|avif|svg\+xml)[;,]/.test(image)) element.setAttribute("src", image);
        else element.removeAttribute('src');
      }
      if (embedded) {
        element.setAttribute("sandbox", "allow-same-origin allow-scripts");
        let renderer = this.frames.get(node.id);
        if (!renderer || renderer.iframe !== element) {
          renderer = new DocumentRenderer(element, command => this.dispatch({ ...command, frames: [{ node: node.id, generation: this.generation }, ...(command.frames || [])] }), this.canControl, this.media, [...this.route, { node: node.id, generation: snapshot.generation }]);
          this.frames.set(node.id, renderer);
        }
        pendingFrames.push(() => renderer.render(node.document));
      } else if (node.media && this.media) {
        this.media.attach(node, element, snapshot.generation, this.route);
      } else if (blocked || ["canvas", "video", "audio"].includes(node.tag)) {
        element.textContent = `Unsupported region: ${node.tag}`;
        element.setAttribute("data-rui-unsupported", node.tag);
      } else if (!(element === doc.activeElement && element.isContentEditable && this.drafts.has(node.id))) {
        const children = (node.children || []).map(child => build(child, depth + 1));
        let cursor = element.firstChild;
        for (const child of children) {
          if (cursor !== child) element.insertBefore(child, cursor);
          cursor = child.nextSibling;
        }
        while (cursor) { const next = cursor.nextSibling; cursor.remove(); cursor = next; }
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
        let cursor = shadow.firstChild;
        for (const child of children) {
          if (child !== cursor) shadow.insertBefore(child, cursor);
          cursor = child.nextSibling;
        }
        while (cursor) { const next = cursor.nextSibling; cursor.remove(); cursor = next; }
        for (const text of node.shadowStyles || []) { const style = doc.createElement("style"); style.textContent = css(text); shadow.append(style); }
      }
      return element;
    };
    const csp = doc.head.querySelector("meta[http-equiv]")?.cloneNode(true);
    const root = build(snapshot.root);
    const head = root.querySelector("head") || root.insertBefore(doc.createElement("head"), root.firstChild);
    if (csp) head.prepend(csp);
    for (const sheet of snapshot.styles) {
      const style = doc.createElement("style");
      style.textContent = css(sheet.text ?? snapshot.styleResources?.[sheet.resource] ?? "");
      head.append(style);
    }
    if (root !== doc.documentElement) doc.replaceChild(root, doc.documentElement);
    this.generation = snapshot.generation;
    this.revision = snapshot.revision;
    for (const render of pendingFrames) await render();
    for (const id of this.frames.keys()) if (!this.nodes.has(id)) this.frames.delete(id);
  }
}
