const HTML = "http://www.w3.org/1999/xhtml";
const SVG = "http://www.w3.org/2000/svg";
const BLOCKED = new Set(["script", "noscript", "iframe", "object", "embed", "base", "meta", "link", "style", "foreignObject", "animate", "set"]);
const SAFE_ATTR = /^(id|class|title|lang|dir|role|aria-[\w-]+|data-[\w-]+|style|width|height|viewBox|d|fill|stroke|stroke-width|transform|x|y|x1|y1|x2|y2|cx|cy|r|rx|ry|points|xmlns|type|name|placeholder|disabled|readonly|required|min|max|step|multiple|rows|cols|colspan|rowspan|alt)$/;

export class DocumentRenderer {
  constructor(iframe) {
    this.iframe = iframe;
    iframe.setAttribute("sandbox", "allow-same-origin");
    this.generation = null;
  }
  async render(snapshot) {
    if (!snapshot.root || JSON.stringify(snapshot).length > 8 * 1024 * 1024) throw Error("Document limit");
    const iframe = this.iframe;
    await new Promise(resolve => {
      iframe.onload = resolve;
      iframe.srcdoc = `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'"></head><body></body></html>`;
    });
    const doc = iframe.contentDocument;
    this.nodes = new Map();
    let count = 0;
    const build = (node, depth = 0) => {
      if (++count > 20000 || depth > 128 || this.nodes.has(node.id)) throw Error("Document structure limit");
      if (typeof node.text === "string") {
        const text = doc.createTextNode(node.text);
        this.nodes.set(node.id, text);
        return text;
      }
      if (![HTML, SVG].includes(node.ns) || !/^[a-zA-Z][\w-]*$/.test(node.tag)) throw Error("Unsupported element");
      const blocked = BLOCKED.has(node.tag);
      const element = doc.createElementNS(blocked ? HTML : node.ns, blocked ? "span" : node.tag);
      this.nodes.set(node.id, element);
      for (const [name, value] of Object.entries(node.attrs || {})) {
        if (SAFE_ATTR.test(name) && typeof value === "string") element.setAttribute(name, value);
      }
      if (node.tag === "img" && /^data:image\/(png|jpeg|gif|webp|avif);base64,/.test(node.attrs?.src)) element.setAttribute("src", node.attrs.src);
      if (blocked || ["canvas", "video"].includes(node.tag)) {
        element.textContent = `Unsupported region: ${node.tag}`;
        element.setAttribute("data-rui-unsupported", node.tag);
      } else for (const child of node.children || []) element.append(build(child, depth + 1));
      if (typeof node.value === "string" && "value" in element) element.value = node.value;
      if (typeof node.checked === "boolean" && "checked" in element) element.checked = node.checked;
      return element;
    };
    const root = build(snapshot.root);
    const head = root.querySelector("head") || root.insertBefore(doc.createElement("head"), root.firstChild);
    head.prepend(doc.head.firstChild.cloneNode(true));
    for (const sheet of snapshot.styles) {
      const style = doc.createElement("style");
      style.textContent = sheet.text;
      head.append(style);
    }
    doc.replaceChild(root, doc.documentElement);
    this.generation = snapshot.generation;
    this.revision = snapshot.revision;
  }
}
