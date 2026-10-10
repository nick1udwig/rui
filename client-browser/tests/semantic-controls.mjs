import { SemanticControls } from "../dist/semantic-controls.js";
import { text } from "../dist/model.js";
import { model } from "./semantic.mjs";
const assert = (ok, message) => {
  if (!ok) throw Error(message);
};
export class Client {
  listeners = new Set();
  held = false;
  ready = 0n;
  pendingCommands = 0;
  commands = [];
  captures = [];
  sequence = 0n;
  constructor(state) {
    this.current = state;
  }
  get state() {
    return structuredClone(this.current);
  }
  get capabilities() {
    return {
      mode: "semanticOnly",
      profiles: [],
      can_control: true,
      actions: ["Activate", "Focus", "ReplaceText", "SetSelection", "SetValue"],
    };
  }
  get hasControl() {
    return this.held && this.ready === this.current.revision;
  }
  subscribe(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }
  emit(event) {
    for (const fn of [...this.listeners]) fn(event);
  }
  semanticReady(revision) {
    assert(revision === this.current.revision, "wrong consumer revision");
    this.ready = revision;
    this.emit({ type: "semanticReady", revision });
  }
  send(action, target, args) {
    assert(
      this.hasControl && !this.pendingCommands,
      "invalid command admission",
    );
    const sequence = ++this.sequence;
    this.pendingCommands = 1;
    this.commands.push({ action, target, args, sequence });
    return sequence;
  }
  activate(id) {
    return this.send("Activate", id);
  }
  activateItem(collection, item, version) {
    return this.send("ActivateItem", item, { collection, version });
  }
  focus(id) {
    return this.send("Focus", id);
  }
  setChecked(id, checked) {
    return this.send("SetValue", id, { checked });
  }
  captureEdit(target) {
    const session = [...this.current.records.values()].find(
      (r) => "Session" in r,
    )?.Session;
    assert(
      session?.focus?.id === target && this.hasControl && !this.pendingCommands,
      "source focus unavailable",
    );
    const m = text(this.current, target);
    const capture = {
      target,
      revision: this.current.revision,
      textVersion: m.text_version,
      content: m.content,
      token: new Uint8Array(32).fill(7),
    };
    this.captures.push(capture);
    return capture;
  }
  commitEdit(capture, content) {
    if (capture.revision !== this.current.revision)
      throw Error("editing context changed");
    return this.send("ReplaceText", capture.target, { capture, content });
  }
  focusConfirmed(target) {
    for (const r of this.current.records.values())
      if ("Session" in r) {
        r.Session.focus = { kind: "Semantic", id: target };
        r.Session.focus_generation += 1n;
      }
    this.pendingCommands = 0;
    this.current.revision += 1n;
    this.ready = 0n;
    this.emit({ type: "applied", revision: this.current.revision });
  }
}
export async function checkSemanticControls() {
  const fixtures = await (await fetch("/wasm/semantic-views.json")).json();
  for (const fixture of fixtures) {
    const state = model(fixture.state),
      root = document.createElement("div"),
      status = document.createElement("p");
    document.body.append(root, status);
    const client = new Client(state),
      controls = new SemanticControls(client, root, status);
    client.emit({ type: "applied", revision: state.revision });
    const buttons = [...root.querySelectorAll("button[data-action]")];
    assert(
      buttons.length > 2 && buttons.every((b) => b.disabled),
      "observer action enabled",
    );
    const field = root.querySelector("textarea[data-authoritative]");
    field.focus();
    field.setSelectionRange(0, 1);
    assert(
      client.commands.length === 0,
      "local observer focus/selection dispatched",
    );
    buttons[0].onclick();
    assert(
      client.commands.length === 0,
      "programmatic observer action bypassed guard",
    );
    client.held = true;
    client.emit({ type: "control", granted: true });
    const edit = root.querySelector('button[data-action="EditDraft"]');
    const target = BigInt(
      edit.closest("[data-semantic-id]").dataset.semanticId,
    );
    edit.click();
    assert(
      client.commands.at(-1).action === "Focus" &&
        !root.querySelector("[data-local-draft]"),
      "draft captured before confirmed source focus",
    );
    client.focusConfirmed(target);
    const draft = root.querySelector("[data-local-draft]");
    assert(draft, "confirmed source focus did not capture draft");
    draft.value = "Local é😀אב";
    draft.dispatchEvent(new Event("input", { bubbles: true }));
    assert(client.commands.length === 1, "local input dispatched");
    const original = client.captures.at(-1),
      source = text(client.current, target).content;
    client.current.revision += 1n;
    client.ready = 0n;
    client.emit({ type: "applied", revision: client.current.revision });
    assert(
      root.querySelector("[data-local-draft]") === draft &&
        draft.value === "Local é😀אב",
      "harmless update rebuilt/discarded original draft",
    );
    root.querySelector('[data-action="ApplyDraft"]').click();
    assert(
      client.commands.length === 1 &&
        root.textContent.includes("Conflicting source context"),
      "stale draft was silently rebased",
    );
    assert(
      text(client.current, target).content === source,
      "stale draft overwrote authoritative value",
    );
    client.held = false;
    client.emit({ type: "control", granted: false });
    assert(
      !root.querySelector("[data-local-draft]") &&
        original.token.every((n) => n === 0) &&
        original.content === "",
      "revocation retained captured content/token",
    );
    client.held = true;
    client.emit({ type: "control", granted: true });
    root.querySelector('[data-action="EditDraft"]').click();
    client.focusConfirmed(target);
    const fresh = root.querySelector("[data-local-draft]");
    fresh.value = "😀".repeat(4097);
    fresh.dispatchEvent(new Event("input", { bubbles: true }));
    assert(
      root.querySelector('[data-action="ApplyDraft"]').disabled &&
        client.commands.length === 2,
      "UTF-8 draft ceiling failed",
    );
    fresh.value = "a".repeat(16384);
    fresh.dispatchEvent(new Event("input", { bubbles: true }));
    assert(
      !root.querySelector('[data-action="ApplyDraft"]').disabled,
      "exact draft ceiling rejected",
    );
    fresh.value = "Explicit Unicode 😀";
    fresh.dispatchEvent(new Event("input", { bubbles: true }));
    root.querySelector('[data-action="ApplyDraft"]').click();
    assert(
      client.commands.at(-1).action === "ReplaceText" &&
        client.commands.at(-1).args.content === fresh.value,
      "explicit draft failed",
    );
    client.emit({
      type: "commandResult",
      sequence: client.sequence,
      status: "OutcomeUnknown",
      rejection: "",
      revision: null,
    });
    assert(
      status.textContent.includes("Ambiguous") &&
        root.querySelector('[data-action="ApplyDraft"]').disabled,
      "unknown result enabled another edit",
    );
    client.emit({ type: "disconnected" });
    assert(
      root.childNodes.length === 0 && status.textContent.includes("unresolved"),
      "disconnect retained unauthorized view or hid ambiguity",
    );
    controls.dispose();
    client.emit({ type: "applied", revision: client.current.revision });
    assert(root.childNodes.length === 0, "disposed listener rendered");
    root.remove();
    status.remove();
  }
  return {
    guardedSemanticDom: true,
    observerNavigation: true,
    explicitSourceFocus: true,
    originalDraftClocks: true,
    revocationPurgesDraft: true,
    ambiguousResults: true,
  };
}
