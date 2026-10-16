// IDs are monotonic across publications, including discarded builds. Versions
// cover public content and private dispatch guards; retired IDs never return.
export class SemanticNodes {
  constructor(previous) {
    this.sequence = previous?.sequence ?? { next: 10 };
    this.keys = new Map(previous?.keys);
    this.versions = new Map(previous?.versions);
    this.targets = new Map();
  }
  id(key) {
    if (key === "root") return 3;
    if (!this.keys.has(key)) {
      if (this.sequence.next >= 1_000_000)
        throw Error("Semantic identity limit; restart host");
      this.keys.set(key, this.sequence.next++);
    }
    return this.keys.get(key);
  }
  node(key, data, handler, guard = "") {
    const id = this.id(key);
    const node = {
      id,
      version: 1,
      role: "label",
      label: "",
      description: "",
      kind: null,
      value: null,
      children: [],
      actions: [],
      disabled: false,
      ...data,
    };
    const signature = JSON.stringify([node, guard]);
    const prior = this.versions.get(id);
    node.version = prior
      ? prior.version + Number(prior.signature !== signature)
      : 1;
    this.versions.set(id, { signature, version: node.version });
    if (handler) this.targets.set(id, { ...handler, node });
    else this.targets.delete(id);
    return node;
  }
  retain(nodes) {
    const live = new Set(nodes.map((node) => node.id));
    for (const [key, id] of this.keys) if (!live.has(id)) this.keys.delete(key);
    for (const map of [this.versions, this.targets])
      for (const id of map.keys()) if (!live.has(id)) map.delete(id);
  }
}
