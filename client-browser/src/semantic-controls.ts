import {
  BrowserClient,
  type BrowserEvent,
  type EditCapture,
} from "./client.js";
import { semantic, text, type Id, type Semantic, type State } from "./model.js";
import { SemanticView, type SemanticViewLimits } from "./semantic.js";
interface Draft {
  capture: EditCapture;
  panel: HTMLElement;
  input: HTMLTextAreaElement;
  apply: HTMLButtonElement;
  message: HTMLElement;
  sequence?: bigint;
  applied?: boolean;
}
interface FocusIntent {
  target: Id;
  sequence: bigint;
}
/** Optional guarded interactions for the local semantic presentation.
 * Navigation/focus in this DOM never dispatches; explicit operations do.
 */
export class SemanticControls {
  private view: SemanticView;
  private state: State | undefined;
  private actions = new Map<Id, HTMLElement>();
  private draft: Draft | undefined;
  private focusIntent: FocusIntent | undefined;
  private unsubscribe: () => void;
  private disposed = false;
  private ambiguous = false;
  constructor(
    private client: BrowserClient,
    private root: HTMLElement,
    private status: HTMLElement,
    limits: SemanticViewLimits = {},
  ) {
    if (client.capabilities.mode !== "semanticOnly")
      throw new Error("semantic-only client required");
    this.view = new SemanticView(root, limits);
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    this.unsubscribe = client.subscribe((event) => this.event(event));
  }
  private say(message: string): void {
    this.status.textContent = message;
  }
  private supported(node: Semantic, action: string): boolean {
    return node.actions.some((a) => a.id === action && a.schema_version === 1);
  }
  private allowed(node: Semantic): boolean {
    return (
      this.client.hasControl &&
      !this.client.pendingCommands &&
      (!node.states.has("Disabled") ||
        node.states.get("Disabled") === "False") &&
      node.sensitivity === "Public"
    );
  }
  private guard(node: Semantic): void {
    const current = this.client.state;
    if (
      !this.state ||
      !current ||
      semantic(current, node.id)?.version !== node.version ||
      !this.allowed(node)
    )
      throw new Error("Source context changed; no command was sent.");
  }
  private operation(node: Semantic, action: () => bigint): void {
    try {
      this.guard(node);
      const sequence = action();
      this.say(
        `Pending command ${sequence}; last source value remains authoritative.`,
      );
      this.refresh();
    } catch (error) {
      this.say(String(error));
      this.refresh();
    }
  }
  private refresh(): void {
    if (this.disposed || !this.state) return;
    const seen = new Set<Id>();
    const capabilities = this.client.capabilities;
    const session = [...this.state.records.values()].find(
      (r) => "Session" in r,
    );
    const owners = new Map<Id, Semantic>();
    for (const record of this.state.records.values())
      if ("Semantic" in record && record.Semantic.collection)
        for (const ref of record.Semantic.collection.items)
          owners.set(ref.id, record.Semantic);
    for (const element of Array.from(
      this.root.querySelectorAll<HTMLElement>("[data-semantic-id]"),
    )) {
      const id = BigInt(element.dataset.semanticId!);
      const node = semantic(this.state, id);
      if (!node) continue;
      seen.add(id);
      let container = this.actions.get(id);
      if (!container) {
        container = element.ownerDocument.createElement("div");
        container.className = "rui-semantic-actions";
        element.insertBefore(
          container,
          element.querySelector(".rui-semantic-info"),
        );
        this.actions.set(id, container);
      }
      // Preserve buttons and handlers by operation key rather than recreate focused DOM.
      const wanted = new Set<string>();
      const button = (
        key: string,
        label: string,
        enabled: boolean,
        run: () => void,
      ) => {
        wanted.add(key);
        let control = container!.querySelector<HTMLButtonElement>(
          `button[data-action="${key}"]`,
        );
        if (!control) {
          control = element.ownerDocument.createElement("button");
          control.type = "button";
          control.dataset.action = key;
          container!.append(control);
        }
        control.textContent = label;
        control.setAttribute(
          "aria-label",
          `${label}: ${node.label ?? node.role}`,
        );
        control.disabled = !enabled;
        control.onclick = run;
      };
      const can = this.allowed(node),
        permitted = (action: string) =>
          can &&
          capabilities.actions.includes(action) &&
          this.supported(node, action);
      const authoritative =
        element.querySelector<HTMLTextAreaElement>(":scope > textarea");
      if (authoritative) authoritative.dataset.authoritative = "true";
      const checkbox = element.querySelector<HTMLInputElement>(
        ":scope > input[type=checkbox]",
      );
      if (checkbox) {
        const boolean =
          node.value && "Boolean" in node.value
            ? node.value.Boolean
            : undefined;
        checkbox.disabled = boolean === undefined || !permitted("SetValue");
        checkbox.onchange = () => {
          const desired = checkbox.checked;
          checkbox.checked = boolean ?? false;
          this.operation(node, () => this.client.setChecked(id, desired));
        };
      }
      if (this.supported(node, "Activate")) {
        const owner = owners.get(id);
        button(
          "Activate",
          node.role === "ListItem" ? "Select source item" : "Activate",
          permitted("Activate"),
          () =>
            this.operation(node, () =>
              owner?.collection
                ? this.client.activateItem(
                    owner.id,
                    id,
                    owner.collection.version,
                  )
                : this.client.activate(id),
            ),
        );
      }
      if (
        node.text &&
        this.supported(node, "ReplaceText") &&
        this.supported(node, "Focus") &&
        text(this.state, id)?.editable
      ) {
        button(
          "EditDraft",
          "Edit draft",
          permitted("Focus") && capabilities.actions.includes("ReplaceText"),
          () => {
            this.operation(node, () => {
              this.purgeDraft();
              const sequence = this.client.focus(id);
              this.focusIntent = { target: id, sequence };
              return sequence;
            });
          },
        );
      }
      for (const control of Array.from(
        container.querySelectorAll<HTMLButtonElement>("button[data-action]"),
      ))
        if (!wanted.has(control.dataset.action!)) control.remove();
      element.dataset.sourceFocused = String(
        !!(session && "Session" in session && session.Session.focus?.id === id),
      );
    }
    for (const [id, element] of this.actions)
      if (!seen.has(id)) {
        element.remove();
        this.actions.delete(id);
      }
    if (this.draft) {
      if (!seen.has(this.draft.capture.target)) {
        this.purgeDraft();
        this.say("Source control was removed; draft retired.");
      } else {
        const draft = this.draft;
        draft.apply.disabled =
          !this.client.hasControl ||
          !!this.client.pendingCommands ||
          draft.sequence !== undefined ||
          draft.applied === true ||
          new TextEncoder().encode(draft.input.value).length > 16 << 10;
        draft.input.readOnly = draft.applied === true;
        draft.input.disabled = draft.sequence !== undefined;
      }
    }
    this.finishFocus();
  }
  private finishFocus(): void {
    const intent = this.focusIntent;
    if (!intent || this.client.pendingCommands || !this.client.hasControl)
      return;
    try {
      const capture = this.client.captureEdit(intent.target);
      this.focusIntent = undefined;
      const doc = this.root.ownerDocument,
        panel = doc.createElement("section"),
        label = doc.createElement("label"),
        input = doc.createElement("textarea"),
        apply = doc.createElement("button"),
        message = doc.createElement("p");
      panel.className = "rui-local-draft";
      label.textContent = "Local draft";
      input.value = capture.content;
      input.rows = 3;
      input.dir = "auto";
      input.maxLength = 16384;
      input.setAttribute(
        "aria-label",
        `Local draft for ${semantic(this.state!, intent.target)?.label ?? "source field"}`,
      );
      input.dataset.localDraft = "true";
      message.setAttribute("role", "status");
      message.textContent =
        "Original source context captured. Apply explicitly; local composition stays in this draft.";
      apply.type = "button";
      apply.textContent = "Apply draft";
      apply.dataset.action = "ApplyDraft";
      label.append(input);
      panel.append(label, apply, message);
      this.root.append(panel);
      this.draft = { capture, panel, input, apply, message };
      const submit = () => {
        const draft = this.draft;
        if (!draft) return;
        try {
          draft.sequence = this.client.commitEdit(
            draft.capture,
            draft.input.value,
          );
          draft.message.textContent =
            "Pending; the source field above remains authoritative.";
          this.refresh();
        } catch (error) {
          draft.message.textContent = `Conflicting source context: ${String(error)}. Original draft retained; start a new explicit edit to capture another context.`;
        }
      };
      apply.onclick = submit;
      input.oninput = () => {
        this.refresh();
        if (new TextEncoder().encode(input.value).length > 16 << 10)
          message.textContent = "Local draft exceeds the 16 KiB commit limit.";
      };
      input.onkeydown = (event) => {
        if (event.ctrlKey && event.key === "Enter" && !event.isComposing) {
          event.preventDefault();
          submit();
        }
      };
      input.focus({ preventScroll: true });
      this.say("Local draft ready; source focus is confirmed.");
    } catch (error) {
      this.focusIntent = undefined;
      this.say(`Source focus was not confirmed: ${String(error)}`);
    }
  }
  private purgeDraft(): void {
    this.focusIntent = undefined;
    if (this.draft) {
      this.draft.capture.token.fill(0);
      this.draft.capture.content = "";
      this.draft.input.value = "";
      this.draft.panel.remove();
      this.draft = undefined;
    }
  }
  private reset(): void {
    this.purgeDraft();
    this.actions.clear();
    this.state = undefined;
    this.view.reset();
  }
  private event(event: BrowserEvent): void {
    if (this.disposed) return;
    if (event.type === "applied" || event.type === "resumed") {
      const state = this.client.state;
      if (!state) return;
      this.state = state;
      this.view.render(state);
      this.refresh();
      this.client.semanticReady(state.revision);
      this.refresh();
    } else if (event.type === "semanticReady" || event.type === "control") {
      if (event.type === "control" && !event.granted) {
        this.purgeDraft();
        this.say("Observer; source mutation is disabled.");
      }
      this.refresh();
    } else if (event.type === "commandResult") {
      const outcome = event.status.toLowerCase();
      this.ambiguous = ["unknown", "outcomeunknown", "outcomeexpired"].includes(
        outcome,
      );
      this.say(
        this.ambiguous
          ? "Ambiguous command outcome. Resolve the original command; do not submit a fresh retry."
          : outcome === "dispatched"
            ? "Command dispatched; awaiting source evidence."
            : `${event.status}${event.rejection ? ": " + event.rejection : ""}`,
      );
      if (this.draft?.sequence === event.sequence) {
        this.draft.message.textContent = this.status.textContent;
        if (outcome !== "dispatched" && !this.ambiguous) {
          this.draft.sequence = undefined;
          this.draft.applied = outcome === "applied";
        }
      }
      if (
        this.focusIntent?.sequence === event.sequence &&
        outcome !== "applied" &&
        outcome !== "dispatched"
      ) {
        this.focusIntent = undefined;
      }
      this.refresh();
    } else if (event.type === "reset" || event.type === "disconnected") {
      this.reset();
      if (event.type === "disconnected")
        this.say(
          this.ambiguous
            ? "Disconnected; original command outcome remains unresolved."
            : "Disconnected; authorized draft and view retired.",
        );
    } else if (event.type === "error" || event.type === "failure") {
      this.purgeDraft();
      this.say(event.type === "error" ? event.code : event.message);
      this.refresh();
    } else if (event.type === "connected" && !event.can_control)
      this.purgeDraft();
  }
  /** Consume an already applied model after page visibility restores. */
  consumeCurrent(): void {
    if (this.disposed) return;
    const state = this.client.state;
    if (!state) return;
    this.state = state;
    this.view.render(state);
    this.client.semanticReady(state.revision);
    this.refresh();
  }
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.unsubscribe();
    this.reset();
    this.view.dispose();
  }
}
