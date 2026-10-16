// Validate once before recursive presentation. Cache key encodings so walking
// ancestors, children and dispatch targets does not repeatedly stringify them.
export function indexTree(nodes) {
  if (!Array.isArray(nodes) || !nodes.length || nodes.length > 1024)
    throw Error("Desktop tree size limit");
  const encodings = new WeakMap(),
    byKey = new Map(),
    parents = new Map();
  const keyOf = (key) => {
    if (
      !Array.isArray(key) ||
      key.length !== 2 ||
      key.some((part) => typeof part !== "string")
    )
      throw Error("Invalid desktop identity");
    if (!encodings.has(key)) encodings.set(key, JSON.stringify(key));
    return encodings.get(key);
  };
  const encoded = (node) => keyOf(node.key);
  for (const node of nodes) {
    const key = encoded(node);
    if (
      byKey.has(key) ||
      !Array.isArray(node.children) ||
      !Array.isArray(node.actions)
    )
      throw Error("Invalid desktop node");
    byKey.set(key, node);
  }
  for (const node of nodes)
    for (const child of node.children) {
      const key = keyOf(child);
      if (!byKey.has(key) || parents.has(key))
        throw Error("Invalid desktop child");
      parents.set(key, node);
    }
  const root = nodes[0];
  for (const node of nodes) {
    const parent = parents.get(encoded(node));
    if (
      (node === root) === !!parent ||
      (node.parent !== undefined &&
        (node.parent === null
          ? !!parent
          : !parent || keyOf(node.parent) !== encoded(parent)))
    )
      throw Error("Invalid desktop parent");
  }
  const seen = new Set();
  const visit = (node, depth) => {
    const key = encoded(node);
    if (depth > 64 || seen.has(key))
      throw Error("Desktop tree depth or cycle limit");
    seen.add(key);
    for (const child of node.children)
      visit(byKey.get(keyOf(child)), depth + 1);
  };
  visit(root, 0);
  if (seen.size !== nodes.length) throw Error("Disconnected desktop tree");
  return {
    byKey,
    keyOf,
    encoded,
    parentOf: (node) => parents.get(encoded(node)),
  };
}
