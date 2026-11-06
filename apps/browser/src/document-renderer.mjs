const HTML = "http://www.w3.org/1999/xhtml";
const SVG = "http://www.w3.org/2000/svg";
const BLOCKED = new Set(["script", "noscript", "iframe", "object", "embed", "base", "meta", "link", "style", "foreignObject", "animate", "set"]);
const SAFE_ATTR = /^(id|class|title|lang|dir|role|aria-[\w-]+|data-[\w-]+|style|width|height|viewBox|d|fill|stroke|stroke-width|transform|x|y|x1|y1|x2|y2|cx|cy|r|rx|ry|points|xmlns|type|name|placeholder|disabled|readonly|required|min|max|step|multiple|rows|cols|colspan|rowspan|alt)$/;

export class DocumentRenderer {
  constructor(iframe, dispatch = () => {}, canControl = () => false, media, frames = []) {
    this.iframe = iframe;
    iframe.setAttribute("sandbox", "allow-same-origin");
    this.generation = null;
    this.dispatch = dispatch;
    this.canControl = canControl;
    this.media = media;
    this.route = frames;
  }
  async render(snapshot) {
    if (!snapshot.root || JSON.stringify(snapshot).length > 8 * 1024 * 1024) throw Error("Document limit");
    const iframe = this.iframe;
    if (snapshot.generation !== this.generation) await new Promise(resolve => {
      iframe.onload = resolve;
      iframe.srcdoc = `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'"></head><body></body></html>`;
    });
    const doc = iframe.contentDocument;
    if (snapshot.generation !== this.generation) {
      const send = (node, action) => {
        if (!this.canControl()) return;
        this.dispatch({ generation: this.generation, revision: this.revision, node, ...action });
      };
      doc.addEventListener("click", event => {
        event.preventDefault();
        const target = event.composedPath().find(node => this.ids?.has(node));
        if (target) send(this.ids.get(target), { action: "click" });
      }, true);
      doc.addEventListener("beforeinput", event => { if (!this.canControl()) event.preventDefault(); }, true);
      doc.addEventListener("change", event => {
        const node = this.ids?.get(event.target);
        if (node && typeof event.target.value === "string") send(node, { action: "value", value: event.target.value, expected: this.values.get(node) });
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
        if (text.data !== node.text) text.data = node.text;
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
        if (SAFE_ATTR.test(name) && typeof value === "string") {
          keep.add(name);
          if (element.getAttribute(name) !== value) element.setAttribute(name, value);
        }
      }
      for (const attr of [...element.attributes]) if (!keep.has(attr.name) && attr.name !== "src") element.removeAttribute(attr.name);
      if (node.tag === "img" && /^data:image\/(png|jpeg|gif|webp|avif);base64,/.test(node.attrs?.src)) element.setAttribute("src", node.attrs.src);
      if (embedded) {
        element.setAttribute("sandbox", "allow-same-origin");
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
      } else {
        const children = (node.children || []).map(child => build(child, depth + 1));
        let cursor = element.firstChild;
        for (const child of children) {
          if (cursor !== child) element.insertBefore(child, cursor);
          cursor = child.nextSibling;
        }
        while (cursor) { const next = cursor.nextSibling; cursor.remove(); cursor = next; }
      }
      if (typeof node.value === "string" && "value" in element && element !== doc.activeElement && element.value !== node.value) element.value = node.value;
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
        for (const css of node.shadowStyles || []) { const style = doc.createElement("style"); style.textContent = css; shadow.append(style); }
      }
      return element;
    };
    const csp = doc.head.querySelector("meta[http-equiv]")?.cloneNode(true);
    const root = build(snapshot.root);
    const head = root.querySelector("head") || root.insertBefore(doc.createElement("head"), root.firstChild);
    if (csp) head.prepend(csp);
    for (const sheet of snapshot.styles) {
      const style = doc.createElement("style");
      style.textContent = sheet.text;
      head.append(style);
    }
    if (root !== doc.documentElement) doc.replaceChild(root, doc.documentElement);
    this.generation = snapshot.generation;
    this.revision = snapshot.revision;
    for (const render of pendingFrames) await render();
    for (const id of this.frames.keys()) if (!this.nodes.has(id)) this.frames.delete(id);
  }
}
