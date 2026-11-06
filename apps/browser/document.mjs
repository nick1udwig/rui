import { randomBytes } from "node:crypto";

const MAX_BYTES = 8 * 1024 * 1024;

// Runs in the source document. IDs survive observations but never navigation.
function observeDocument(key) {
  const state = globalThis[key] ||= { nodes: new WeakMap(), next: 1 };
  const id = (node) => {
    if (!state.nodes.has(node)) state.nodes.set(node, state.next++);
    return state.nodes.get(node);
  };
  const unsupported = [];
  let count = 0;
  const visit = (node, depth = 0) => {
    if (++count > 20000 || depth > 128) throw Error("Document limit");
    if (node.nodeType === 3) return { id: id(node), text: node.data };
    if (node.nodeType !== 1) return null;
    const tag = node.localName;
    if (["script", "noscript", "base", "meta", "link", "style"].includes(tag)) return null;
    const attrs = Object.fromEntries([...node.attributes].map(a => [a.name, a.value]));
    if (tag === "img") attrs.src = node.currentSrc || node.src;
    const result = { id: id(node), tag, ns: node.namespaceURI, attrs, children: [] };
    if (node instanceof HTMLInputElement || node instanceof HTMLTextAreaElement || node instanceof HTMLSelectElement) {
      result.value = node.type === "password" ? "" : node.value;
      result.checked = !!node.checked;
      if (node.type === "password") delete attrs.value;
    }
    if (["canvas", "video", "iframe", "object", "embed"].includes(tag)) {
      unsupported.push({ id: result.id, kind: tag });
      return result;
    }
    result.children = [...node.childNodes].map(n => visit(n, depth + 1)).filter(Boolean);
    if (node.shadowRoot) {
      unsupported.push({ id: result.id, kind: "shadow-root" });
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
    this.page = page;
    this.key = `__rui_document_${randomBytes(16).toString("hex")}`;
    this.generation = randomBytes(16).toString("hex");
    this.revision = 0;
    this.assets = new Map();
    this.navigation = (frame) => {
      if (frame === page.mainFrame()) {
        this.generation = randomBytes(16).toString("hex");
        this.assets.clear();
      }
    };
    page.on("framenavigated", this.navigation);
  }
  dispose() { this.page.off("framenavigated", this.navigation); this.assets.clear(); }
  async asset(url) {
    if (!/^https?:/.test(url)) return /^data:image\/(png|jpeg|gif|webp);base64,/.test(url) && url.length < MAX_BYTES ? url : "";
    if (this.assets.has(url)) return this.assets.get(url);
    const response = await this.page.context().request.get(url, { timeout: 3000, maxRedirects: 5 });
    try {
      const type = (response.headers()["content-type"] || "").split(";")[0];
      if (!/^(image\/(png|jpeg|gif|webp|avif)|font\/(woff2?|ttf|otf)|application\/font-woff)$/.test(type)) return "";
      const body = await response.body();
      if (body.length > MAX_BYTES || this.assets.size >= 128) throw Error("Asset limit");
      const data = `data:${type};base64,${body.toString("base64")}`;
      if ([...this.assets.values()].reduce((n, v) => n + v.length, data.length) > MAX_BYTES) throw Error("Asset cache limit");
      this.assets.set(url, data);
      return data;
    } finally { await response.dispose(); }
  }
  async capture() {
    const generation = this.generation;
    const result = await this.page.evaluate(observeDocument, this.key);
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
    const visit = async (node) => {
      if (node.tag === "img") node.attrs.src = await this.asset(new URL(node.attrs.src, result.base).href).catch(() => "");
      if (node.attrs?.style) node.attrs.style = await rewrite(node.attrs.style, result.base);
      for (const child of node.children || []) await visit(child);
    };
    await visit(result.root);
    if (generation !== this.generation) throw Error("Navigation during capture");
    const snapshot = { ...result, generation, revision: ++this.revision };
    if (Buffer.byteLength(JSON.stringify(snapshot)) > MAX_BYTES) throw Error("Snapshot limit");
    return snapshot;
  }
}
