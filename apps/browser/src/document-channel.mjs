export const DOCUMENT_PROFILE = "browser.document.v1";
const LIMIT = 8 * 1024 * 1024;
const offers = entries => Object.fromEntries(entries.map(([key, data]) => [key, { bytes: new TextEncoder().encode(data).length }]));
function catalog(node) {
  return { ...node,
    children: node.children?.map(catalog),
    shadow: node.shadow?.map(catalog),
    ...(node.document ? { document: { ...node.document, root: catalog(node.document.root), resources: offers(Object.entries(node.document.resources || {})), styleResources: offers(Object.entries(node.document.styleResources || {})) } } : {}),
  };
}

function flatten(root) {
  const records = new Map();
  const visit = (node, depth) => {
    if (depth > 128 || records.size >= 20000 || !Number.isSafeInteger(node.id) || node.id < 1 || records.has(node.id)) throw Error("Document identity limit");
    const { children, ...record } = node;
    records.set(node.id, { ...record, children: children?.map(child => child.id) });
    for (const child of children || []) visit(child, depth + 1);
  };
  visit(root, 0);
  return records;
}

export class DocumentPublisher {
  publish(snapshot) {
    const records = flatten(catalog(snapshot.root));
    const previous = this.snapshot?.generation === snapshot.generation ? this.records : undefined;
    const { root, ...metadata } = snapshot;
    const packet = {
      profile: DOCUMENT_PROFILE,
      ...metadata,
      root: root.id,
      base: previous ? this.snapshot.revision : 0,
      put: [...records.values()].filter(record => JSON.stringify(record) !== JSON.stringify(previous?.get(record.id))),
      remove: previous ? [...previous.keys()].filter(id => !records.has(id)) : [],
    };
    if (previous && JSON.stringify(this.snapshot.styles) === JSON.stringify(snapshot.styles)) delete packet.styles;
    packet.retireResources = previous ? Object.keys(this.snapshot.resources || {}).filter(key => !(key in (snapshot.resources || {}))) : [];
    packet.retireStyles = previous ? Object.keys(this.snapshot.styleResources || {}).filter(key => !(key in (snapshot.styleResources || {}))) : [];
    packet.resources = offers(Object.entries(snapshot.resources || {}).filter(([key, data]) => !previous || this.snapshot.resources?.[key] !== data));
    packet.styleResources = offers(Object.entries(snapshot.styleResources || {}).filter(([key, data]) => !previous || this.snapshot.styleResources?.[key] !== data));
    if (JSON.stringify(packet).length > LIMIT) throw Error("Document packet limit");
    this.snapshot = snapshot;
    this.records = records;
    return packet;
  }
}

export class DocumentReceiver {
  apply(packet) {
    if (JSON.stringify(packet).length > LIMIT || packet.profile !== DOCUMENT_PROFILE || !Number.isSafeInteger(packet.revision) || packet.revision < 1 || typeof packet.generation !== "string" || packet.generation.length !== 32) throw Error("Document packet rejected");
    if (packet.base && (packet.base !== this.revision || packet.generation !== this.generation || packet.revision <= packet.base)) throw Error("Document checkpoint required");
    const records = packet.base ? new Map(this.records) : new Map();
    for (const id of packet.remove) records.delete(id);
    const seen = new Set();
    for (const record of packet.put) {
      if (seen.has(record.id)) throw Error("Duplicate document record");
      seen.add(record.id);
      records.set(record.id, record);
    }
    const visited = new Set();
    const build = (id, depth) => {
      if (depth > 128 || visited.size >= 20000 || visited.has(id) || !records.has(id)) throw Error("Invalid document graph");
      visited.add(id);
      const node = records.get(id);
      return { ...structuredClone(node), children: node.children?.map(child => build(child, depth + 1)) };
    };
    const root = build(packet.root, 0);
    if (visited.size !== records.size) throw Error("Unreachable document records");
    const styles = packet.styles ?? (packet.base ? this.styles : undefined);
    if (!Array.isArray(styles)) throw Error("Document styles missing");
    const resources = { ...(packet.base ? this.resources : {}), ...packet.resources };
    const styleResources = { ...(packet.base ? this.styleResources : {}), ...packet.styleResources };
    for (const [kind, target] of [['retireResources', resources], ['retireStyles', styleResources]]) {
      if (!Array.isArray(packet[kind] || [])) throw Error('Invalid resource retirement');
      for (const key of packet[kind] || []) delete target[key];
    }
    const { put, remove, ...metadata } = packet;
    const snapshot = { ...metadata, styles, styleResources, resources, root };
    if (JSON.stringify(snapshot).length > LIMIT) throw Error("Retained document limit");
    this.records = records;
    this.revision = packet.revision;
    this.generation = packet.generation;
    this.styles = styles;
    this.resources = resources;
    this.styleResources = styleResources;
    return snapshot;
  }
}
