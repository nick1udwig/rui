import { SemanticView } from "../dist/semantic.js";
const assert = (condition, message) => {
  if (!condition) throw Error(message);
};
const clocks = new Set([
  "revision",
  "id",
  "version",
  "text_version",
  "focus_generation",
  "reading_order",
  "total_count",
  "anchor",
  "focus",
  "stacking_order",
  "Integer",
]);
export function model(value, key = "") {
  if (typeof value === "string" && clocks.has(key) && /^-?\d+$/.test(value))
    return BigInt(value);
  if (Array.isArray(value))
    return value.map((v) =>
      key === "retired" || key === "scalar_clusters" ? BigInt(v) : model(v),
    );
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([k, v]) => [
        k,
        k === "records"
          ? new Map(Object.entries(v).map(([id, r]) => [BigInt(id), model(r)]))
          : k === "states"
            ? new Map(Object.entries(v))
            : model(v, k),
      ]),
    );
  return value;
}
export async function checkSemanticView() {
  const fixtures = await (await fetch("/wasm/semantic-views.json")).json();
  const root = document.createElement("div");
  document.body.append(root);
  const view = new SemanticView(root);
  for (const fixture of fixtures) {
    const state = model(fixture.state);
    view.reset();
    view.render(state);
    assert(
      root.querySelectorAll("[data-semantic-id]").length >= 5,
      fixture.application + " not useful",
    );
    assert(root.querySelectorAll("textarea").length >= 1, "text model omitted");
    assert(!root.querySelector("canvas"), "semantic view constructed a canvas");
    for (const record of state.records.values())
      if ("Semantic" in record) {
        const n = record.Semantic;
        if (n.text) {
          const t = state.records.get(n.text.id)?.Text;
          if (t?.status === "Redacted")
            assert(root.textContent.includes("redacted"), "redaction omitted");
        }
      }
    const textarea = root.querySelector("textarea");
    textarea.focus();
    textarea.setSelectionRange(1, 2);
    const element = textarea.closest("[data-semantic-id]"),
      id = element.dataset.semanticId;
    const renamed = structuredClone(state);
    for (const record of renamed.records.values())
      if ("Semantic" in record)
        record.Semantic.label = '<img src="https://invalid/rui"> renamed אב 😀';
    view.render(renamed);
    assert(
      root.querySelector(`[data-semantic-id="${id}"] textarea`) === textarea,
      "stable identity rebuilt DOM",
    );
    assert(
      document.activeElement === textarea &&
        textarea.selectionStart === 1 &&
        textarea.selectionEnd === 2,
      "harmless update lost local focus/selection",
    );
    assert(
      !root.querySelector("img,script,iframe,a"),
      "remote string became executable markup/URL",
    );
    const privateNode = [...renamed.records.values()].find(
      (r) => "Semantic" in r && r.Semantic.text?.id === undefined,
    );
    assert(privateNode, "semantic mapping missing");
    const removed = structuredClone(renamed);
    const target = BigInt(id);
    removed.records.delete(target);
    for (const record of removed.records.values())
      if ("Semantic" in record)
        record.Semantic.children = record.Semantic.children.filter(
          (r) => r.id !== target,
        );
    view.render(removed);
    assert(
      !root.querySelector(`[data-semantic-id="${id}"]`),
      "removed identity retained",
    );
    const partial = structuredClone(state);
    const list = [...partial.records.values()].find(
      (r) => "Semantic" in r && r.Semantic.collection,
    )?.Semantic;
    if (list) {
      list.collection.total_count = 1000n;
      list.collection.materialization = "Partial";
      view.render(partial);
      assert(
        root.textContent.includes("of 1000; partial collection"),
        "partial list became empty/full",
      );
    }
    const reordered = structuredClone(state);
    const owner = [...reordered.records.values()].find(
      (r) => "Semantic" in r && r.Semantic.children.length > 1,
    )?.Semantic;
    if (owner) {
      const refs = [...owner.children].reverse();
      for (const [index, ref] of refs.entries())
        reordered.records.get(ref.id).Semantic.reading_order = BigInt(index);
      view.render(reordered);
      const body = root.querySelector(
        `[data-semantic-id="${owner.id}"] > .rui-semantic-children`,
      );
      assert(
        [...body.children].map((e) => e.dataset.semanticId).join(",") ===
          refs.map((ref) => String(ref.id)).join(","),
        "reading order ignored",
      );
    }
    // Fresh source identities and arbitrary geometry do not affect role mapping.
    const changed = structuredClone(state);
    const translate = (r) => ({ ...r, id: r.id + 100000n });
    changed.records = new Map(
      [...changed.records].map(([key, r]) => {
        const n = Object.values(r)[0];
        n.id += 100000n;
        for (const field of [
          "children",
          "items",
          "selected_items",
          "bindings",
          "semantics",
          "scenes",
        ])
          if (Array.isArray(n[field])) n[field] = n[field].map(translate);
        for (const field of [
          "text",
          "parent",
          "modal_for",
          "semantic_root",
          "scene_root",
          "focus",
          "focus_surface",
          "semantic_group",
          "surface",
        ])
          if (n[field]) n[field] = translate(n[field]);
        if (n.collection) {
          n.collection.items = n.collection.items.map(translate);
          n.collection.selected_items =
            n.collection.selected_items.map(translate);
        }
        if (n.relations)
          n.relations = n.relations.map((relation) => ({
            ...relation,
            targets: relation.targets.map(translate),
          }));
        if (n.desktop_rect)
          n.desktop_rect = { x: 12345, y: -987, width: 1, height: 1 };
        return [key + 100000n, r];
      }),
    );
    view.reset();
    view.render(changed);
    assert(
      root.querySelectorAll("textarea").length >= 1,
      "new IDs/geometry broke mapping",
    );
    for (const record of changed.records.values())
      if ("Surface" in record) record.Surface.mapped = false;
    view.render(changed);
    assert(
      root.querySelectorAll("[data-semantic-id]").length === 0,
      "unmapped surface displayed",
    );
  }
  const limits = new SemanticView(document.createElement("div"), { nodes: 1 });
  let failed = false;
  try {
    limits.render(model(fixtures[0].state));
  } catch {
    failed = true;
  }
  assert(failed, "DOM work limit bypassed");
  limits.dispose();
  view.dispose();
  assert(root.children.length === 0, "disposal retained authorized content");
  root.remove();
  return {
    semanticApplications: 2,
    stableSemanticDom: true,
    readOnly: true,
    remoteStringSafety: true,
    partialCollections: true,
    localFocus: true,
    geometryIndependent: true,
    boundedDom: true,
  };
}
