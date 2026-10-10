import {
  semantic,
  text,
  type Id,
  type Semantic,
  type State,
  type Ref,
} from "./model.js";

export interface SemanticViewLimits {
  nodes?: number;
  depth?: number;
  textBytes?: number;
}
interface Entry {
  role: Semantic["role"];
  element: HTMLElement;
  label: HTMLElement;
  info: HTMLElement;
  body: HTMLElement;
  control?: HTMLInputElement | HTMLTextAreaElement;
}
let serial = 0;
/** Bounded local read-only presentation of authorized semantic ownership.
 * Source geometry, labels, action targets and GTK classes never determine mapping.
 */
export class SemanticView {
  private entries = new Map<Id, Entry>();
  private prefix = `rui-semantic-${++serial}-`;
  private limit: Required<SemanticViewLimits>;
  private disposed = false;
  constructor(
    private root: HTMLElement,
    limits: SemanticViewLimits = {},
  ) {
    this.limit = {
      nodes: limits.nodes ?? 1024,
      depth: limits.depth ?? 64,
      textBytes: limits.textBytes ?? 512 << 10,
    };
    if (
      !Number.isSafeInteger(this.limit.nodes) ||
      this.limit.nodes < 1 ||
      this.limit.nodes > 4096 ||
      !Number.isSafeInteger(this.limit.depth) ||
      this.limit.depth < 1 ||
      this.limit.depth > 128 ||
      !Number.isSafeInteger(this.limit.textBytes) ||
      this.limit.textBytes < 1 ||
      this.limit.textBytes > 1 << 20
    )
      throw new RangeError("semantic view limits");
    root.classList.add("rui-semantic");
  }
  private entry(node: Semantic): Entry {
    let existing = this.entries.get(node.id);
    if (existing?.role === node.role) return existing;
    existing?.element.remove();
    const doc = this.root.ownerDocument;
    const element = doc.createElement(
      node.role === "Root" || node.role === "Group" ? "section" : "div",
    );
    element.className = `rui-semantic-node rui-role-${node.role.toLowerCase()}`;
    element.id = this.prefix + node.id;
    const label = doc.createElement(
      node.role === "Root" || node.role === "Group" ? "h2" : "span",
    );
    label.id = element.id + "-name";
    label.dir = "auto";
    const info = doc.createElement("p");
    info.id = element.id + "-info";
    info.className = "rui-semantic-info";
    info.dir = "auto";
    const body = doc.createElement("div");
    body.className = "rui-semantic-children";
    element.append(label, info, body);
    const entry: Entry = { role: node.role, element, label, info, body };
    if (node.role === "TextField") {
      const control = doc.createElement("textarea");
      control.readOnly = true;
      control.setAttribute("aria-labelledby", label.id);
      control.setAttribute("aria-describedby", info.id);
      control.rows = 2;
      control.dir = "auto";
      element.insertBefore(control, info);
      entry.control = control;
    } else if (node.role === "Checkbox") {
      const control = doc.createElement("input");
      control.type = "checkbox";
      control.disabled = true;
      control.setAttribute("aria-labelledby", label.id);
      control.setAttribute("aria-describedby", info.id);
      element.insertBefore(control, info);
      entry.control = control;
      element.tabIndex = 0;
    } else if (
      node.role === "Button" ||
      node.role === "ListItem" ||
      node.role === "CustomCanvas"
    )
      element.tabIndex = 0;
    if (node.role === "List") element.setAttribute("role", "list");
    if (node.role === "ListItem") element.setAttribute("role", "listitem");
    this.entries.set(node.id, entry);
    return entry;
  }
  render(state: State): void {
    if (this.disposed) throw new Error("semantic view disposed");
    const active = this.root.ownerDocument.activeElement;
    const seen = new Set<Id>(),
      plan: { node: Semantic; parent: Id | null; modal: boolean }[] = [];
    let bytes = 0;
    let work = state.records.size;
    const encode = new TextEncoder();
    const order = (refs: readonly Ref[]) =>
      refs
        .map((ref, index) => ({ ref, index, node: semantic(state, ref.id) }))
        .sort((a, b) => {
          const ar = a.node?.reading_order,
            br = b.node?.reading_order;
          if (ar === null || ar === undefined)
            return br === null || br === undefined ? a.index - b.index : 1;
          if (br === null || br === undefined) return -1;
          return ar < br ? -1 : ar > br ? 1 : a.index - b.index;
        });
    const visit = (
      ref: Ref,
      parent: Id | null,
      depth: number,
      modal: boolean,
    ) => {
      if (ref.kind !== "Semantic" || seen.has(ref.id)) return;
      const node = semantic(state, ref.id);
      if (!node) return;
      if (depth > this.limit.depth || plan.length >= this.limit.nodes)
        throw new RangeError("semantic materialization limit");
      work +=
        node.children.length +
        node.actions.length +
        node.relations.reduce((count, r) => count + r.targets.length, 0) +
        (node.collection?.items.length ?? 0);
      if (work > this.limit.nodes * 16)
        throw new RangeError("semantic update work limit");
      seen.add(node.id);
      const model = text(state, node.id);
      for (const string of [
        node.label,
        node.description,
        model?.content,
        node.value && "Text" in node.value ? node.value.Text : null,
      ])
        if (string) bytes += encode.encode(string).length;
      if (bytes > this.limit.textBytes)
        throw new RangeError("semantic text limit");
      plan.push({ node, parent, modal });
      for (const { ref } of order(node.children))
        visit(ref, node.id, depth + 1, false);
    };
    try {
      if (state.records.size > this.limit.nodes * 8)
        throw new RangeError("semantic record work limit");
      let surfaces = false;
      for (const record of state.records.values())
        if ("Surface" in record) {
          surfaces = true;
          if (record.Surface.mapped && record.Surface.semantic_root)
            visit(
              record.Surface.semantic_root,
              null,
              1,
              record.Surface.modal_for !== null,
            );
        }
      // Resource-less semantic projections may omit surfaces. Preserve ownership.
      if (!surfaces) {
        const children = new Set<Id>();
        for (const record of state.records.values())
          if ("Semantic" in record)
            for (const ref of record.Semantic.children) children.add(ref.id);
        const roots = [...state.records.values()].flatMap((r) =>
          "Semantic" in r && !children.has(r.Semantic.id)
            ? [{ kind: "Semantic" as const, id: r.Semantic.id }]
            : [],
        );
        for (const { ref } of order(roots)) visit(ref, null, 1, false);
      }
    } catch (error) {
      this.reset();
      const message = this.root.ownerDocument.createElement("p");
      message.setAttribute("role", "status");
      message.textContent = `Semantic view unavailable: ${error instanceof Error ? error.message : String(error)}. Source content exceeds the local display limit.`;
      this.root.append(message);
      throw error;
    }
    for (const { node, modal } of plan) {
      const entry = this.entry(node),
        model = text(state, node.id),
        info: string[] = [];
      entry.label.textContent =
        node.label ??
        (node.sensitivity === "Redacted" ? "Redacted control" : node.role);
      entry.element.dataset.semanticId = String(node.id);
      if (modal) {
        entry.element.setAttribute("role", "dialog");
        // Source modality does not make the observer's local document modal.
        entry.element.setAttribute("aria-modal", "false");
        info.push("Source modal window; local navigation remains available");
      } else if (node.role === "Root" || node.role === "Group")
        entry.element.setAttribute("role", "group");
      if (!modal) entry.element.removeAttribute("aria-modal");
      entry.element.setAttribute("aria-labelledby", entry.label.id);
      entry.element.setAttribute("aria-describedby", entry.info.id);
      entry.element.removeAttribute("aria-controls");
      entry.control?.setAttribute("aria-labelledby", entry.label.id);
      entry.control?.setAttribute("aria-describedby", entry.info.id);
      if (node.description) info.push(node.description);
      if (node.completeness !== "Full")
        info.push(`Content ${node.completeness.toLowerCase()}`);
      if (node.sensitivity !== "Public") info.push(node.sensitivity);
      for (const [key, value] of node.states)
        info.push(`${key}: ${value.toLowerCase()}`);
      if (node.collection) {
        const c = node.collection;
        info.push(
          `${c.items.length} materialized items${c.total_count === null ? ", total unknown" : ` of ${c.total_count}`}; ${c.materialization.toLowerCase()} collection`,
        );
        if (c.selected_items.length)
          info.push(`${c.selected_items.length} source selected`);
      }
      if (model) {
        if (model.status !== "Available")
          info.push(`Text ${model.status.toLowerCase()}`);
        if (
          entry.control instanceof
          this.root.ownerDocument.defaultView!.HTMLTextAreaElement
        ) {
          const content =
            model.status === "Available" ? (model.content ?? "") : "";
          if (entry.control.value !== content) entry.control.value = content;
          entry.control.setAttribute("aria-readonly", "true");
        } else if (model.status === "Available" && model.content !== null)
          info.push(model.content);
      }
      if (
        entry.control instanceof
        this.root.ownerDocument.defaultView!.HTMLInputElement
      ) {
        if (node.value && "Boolean" in node.value) {
          entry.control.checked = node.value.Boolean;
          entry.control.indeterminate =
            node.states.get("Checked") === "Unknown";
        } else {
          entry.control.checked = node.states.get("Checked") === "True";
          entry.control.indeterminate =
            node.states.get("Checked") === "Unknown" ||
            node.states.get("Checked") === undefined;
          info.push(
            node.value
              ? `Value: ${String(Object.values(node.value)[0])}`
              : "Value unavailable",
          );
        }
      } else if (node.value) info.push(String(Object.values(node.value)[0]));
      const unsupported = node.actions.filter(
        (a) =>
          a.schema_version !== 1 ||
          ![
            "Activate",
            "Focus",
            "SetValue",
            "ReplaceText",
            "SetSelection",
            "ScrollTo",
          ].includes(a.id),
      );
      if (unsupported.length) info.push("Unsupported advertised actions");
      if (node.role === "CustomCanvas")
        info.push(
          "Informational canvas semantics; visual content unavailable here",
        );
      entry.info.textContent = info.join(" · ");
    }
    // Relationships only reference already materialized authorized nodes.
    const placed = new Set<Id>(),
      positions = new Map<HTMLElement, number>();
    for (const { node, parent } of plan) {
      const entry = this.entries.get(node.id)!;
      for (const relation of node.relations) {
        const names = relation.targets.flatMap((ref) =>
          this.entries.has(ref.id) && seen.has(ref.id)
            ? [this.entries.get(ref.id)!.label.id]
            : [],
        );
        if (relation.kind === "LabelledBy" && names.length)
          entry.control?.setAttribute("aria-labelledby", names.join(" "));
        if (relation.kind === "Controls") {
          const targets = relation.targets.flatMap((ref) =>
            seen.has(ref.id) ? [this.entries.get(ref.id)!.element.id] : [],
          );
          if (targets.length)
            entry.element.setAttribute("aria-controls", targets.join(" "));
        }
        if (relation.kind === "LabelFor" || relation.kind === "Describes")
          for (const ref of relation.targets) {
            const target = seen.has(ref.id)
              ? this.entries.get(ref.id)
              : undefined;
            if (!target) continue;
            if (relation.kind === "LabelFor") {
              target.element.setAttribute("aria-labelledby", entry.label.id);
              target.control?.setAttribute("aria-labelledby", entry.label.id);
            } else {
              const description =
                target.info.id + " " + entry.label.id + " " + entry.info.id;
              target.element.setAttribute("aria-describedby", description);
              target.control?.setAttribute("aria-describedby", description);
            }
          }
      }
      const group =
        node.semantic_group && placed.has(node.semantic_group.id)
          ? this.entries.get(node.semantic_group.id)
          : undefined;
      const container =
        group && (group.role === "Root" || group.role === "Group")
          ? group.body
          : parent === null
            ? this.root
            : this.entries.get(parent)!.body;
      const position = positions.get(container) ?? 0;
      const next = container.children[position];
      if (next !== entry.element)
        container.insertBefore(entry.element, next ?? null);
      positions.set(container, position + 1);
      placed.add(node.id);
    }
    for (const [id, entry] of this.entries)
      if (!seen.has(id)) {
        entry.element.remove();
        this.entries.delete(id);
      }
    if (
      active instanceof this.root.ownerDocument.defaultView!.HTMLElement &&
      active.isConnected &&
      this.root.contains(active) &&
      this.root.ownerDocument.activeElement !== active
    )
      active.focus({ preventScroll: true });
  }
  reset(): void {
    this.entries.clear();
    this.prefix = `rui-semantic-${++serial}-`;
    this.root.replaceChildren();
  }
  dispose(): void {
    if (this.disposed) return;
    this.reset();
    this.disposed = true;
  }
}
