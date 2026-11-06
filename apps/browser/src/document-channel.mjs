export const DOCUMENT_PROFILE = "browser.document.v1";
const LIMIT = 8 * 1024 * 1024;

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
    const records = flatten(snapshot.root);
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
      return { ...node, children: node.children?.map(child => build(child, depth + 1)) };
    };
    const root = build(packet.root, 0);
    if (visited.size !== records.size) throw Error("Unreachable document records");
    const snapshot = { ...packet, root };
    if (JSON.stringify(snapshot).length > LIMIT) throw Error("Retained document limit");
    this.records = records;
    this.revision = packet.revision;
    this.generation = packet.generation;
    return snapshot;
  }
}
