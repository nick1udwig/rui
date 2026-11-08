import { BrowserClient, type Semantic, type State } from "@rui/client-browser";
import "./style.css";
import { DocumentView } from "./document-view.mjs";
const workspace =
  document.documentElement.dataset.workspace === "desktop"
    ? "desktop"
    : "browser";
const appBase = new URL(document.baseURI);
const deviceKey =
  appBase.pathname === "/" ? "host" : `host:${appBase.pathname}`;
const websocketURL = new URL("rui", appBase);
websocketURL.protocol = appBase.protocol === "https:" ? "wss:" : "ws:";
const $ = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const status = $("status"),
  view = $("view"),
  connection = $("connection");
const connect = $<HTMLButtonElement>("connect"),
  control = $<HTMLButtonElement>("control"),
  release = $<HTMLButtonElement>("release"),
  retry = $<HTMLButtonElement>("retry");
type Device = { client: string; credential: string };
const hex = (bytes: Uint8Array) =>
  Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
const bytes = (value: string) =>
  Uint8Array.from(value.match(/../g)!, (n) => parseInt(n, 16));
let device: Device | undefined,
  client: BrowserClient | undefined,
  authority = 0,
  connecting = false,
  staging = false,
  connected = false,
  uncertainAction = false;
const db = await new Promise<IDBDatabase>((resolve, reject) => {
  const request = indexedDB.open("rui-browser", 1);
  request.onupgradeneeded = () => request.result.createObjectStore("devices");
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
});
function storage(value?: Device | null): Promise<Device | undefined> {
  return new Promise((resolve, reject) => {
    const transaction = db.transaction(
      "devices",
      value === undefined ? "readonly" : "readwrite",
    );
    const store = transaction.objectStore("devices");
    const request =
      value === undefined
        ? store.get(deviceKey)
        : value === null
          ? store.delete(deviceKey)
          : store.put(value, deviceKey);
    transaction.oncomplete = () =>
      resolve(request.result as Device | undefined);
    transaction.onerror = () => reject(transaction.error);
  });
}
async function api(path: string, data: unknown, authenticated = false, signal?: AbortSignal) {
  const response = await fetch(new URL(`api/${path}`, appBase), {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      ...(authenticated && device
        ? {
            Authorization: `Bearer ${device.credential}`,
            "X-RUI-Client": device.client,
          }
        : {}),
    },
    body: JSON.stringify(data),
    cache: "no-store",
    signal,
  });
  if (response.ok && response.headers.get("content-type") === "application/vnd.rui.media") return response;
  const result = await response.json();
  if (!response.ok) throw Object.assign(Error(result.error), { code: result.code });
  return result;
}
let toastTimer: ReturnType<typeof setTimeout> | undefined;
function say(error: unknown) {
  status.textContent = String(error);
  status.classList.add("toast-visible");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => status.classList.remove("toast-visible"), 5000);
}
type Row = {
  element: HTMLElement;
  label: HTMLElement;
  description: HTMLElement;
  children?: HTMLElement;
  input?: HTMLInputElement | HTMLTextAreaElement;
  button?: HTMLButtonElement;
  apply?: HTMLButtonElement;
  message?: HTMLElement;
  node: Semantic;
  dirty?: { version: bigint; authority: number };
  submitted?: bigint;
  kind: string;
  filter?: HTMLInputElement;
};
const rows = new Map<bigint, Row>();
type DocumentCommand = { generation: string; frames?: { generation: string }[]; action: string };
const commandGeneration = (command: DocumentCommand) => command.frames?.[0]?.generation ?? command.generation;
let documentTarget: Semantic | undefined;
let documentCommands: DocumentCommand[] = [];
function drainDocumentCommands() {
  if (!client?.hasControl || client.pendingCommands || !documentTarget || !documentView?.generation) return;
  documentCommands = documentCommands.filter(command => commandGeneration(command) === documentView.generation);
  const command = documentCommands[0];
  if (!command || documentTarget.description !== commandGeneration(command)) return;
  documentCommands.shift();
  try { client.setBrowserValue(documentTarget.id, JSON.stringify(command)); refreshControls(); buttons(); }
  catch (error) { documentCommands = []; say(error); }
}
const documentContainer = document.createElement("section");
documentContainer.id = "page";
$("reader").before(documentContainer);
const slots = {
  address: $("address-slot"),
  reload: $("reload-slot"),
  nav: $("nav-slot"),
  tabs: $("tabs-slot"),
  newTab: $("new-tab-slot"),
  menu: $("menu-slot"),
  prompt: $("prompt-slot"),
  store: $("chrome-store"),
};
const tabsSheet = $<HTMLDialogElement>("tabs-sheet"),
  menuSheet = $<HTMLDialogElement>("menu-sheet");
const documentView = workspace === "browser" ? new DocumentView(documentContainer, api, command => {
  if (!client?.hasControl) return;
  if (documentCommands.length >= 16) { say("Page input queue full. Wait for the host."); return; }
  const action = command as DocumentCommand;
  if (action.action === "resize") documentCommands = documentCommands.filter(queued => queued.action !== "resize");
  documentCommands.push(action);
  drainDocumentCommands();
}, () => !!client?.hasControl, say, drainDocumentCommands) : undefined;
if (workspace === "browser") {
  $("sound-slot").append(documentView!.soundButton);
  documentView!.soundButton.classList.add("icon-button");
  documentView!.soundButton.dataset.icon = "sound";
  documentView!.soundButton.setAttribute("aria-label", "Enable page sound");
  documentView!.soundButton.addEventListener("click", () => documentView!.soundButton.setAttribute("aria-pressed", "true"));
  slots.menu.after(document.querySelector(".session-details")!);
  $("address-bar").append(document.querySelector('nav[aria-label="Session"]')!);
  $("menu-session").append(document.querySelector(".host-dot")!, connection, release);
  release.textContent = "Release control";
  release.addEventListener("click", () => menuSheet.close());
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", "#ffffff");
  for (const sheet of [tabsSheet, menuSheet]) {
    sheet.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target === sheet || target.closest(".sheet-done")) sheet.close();
    });
  }
  $("tabs-button").onclick = () => tabsSheet.showModal();
  $("menu-button").onclick = () => menuSheet.showModal();
  $("open-reader").onclick = () => {
    menuSheet.close();
    $("reader").scrollIntoView({ behavior: "smooth" });
  };
  slots.tabs.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest("button");
    if (button && !button.disabled && button.getAttribute("aria-label") !== "Close tab") tabsSheet.close();
  });
  slots.newTab.addEventListener("click", () => tabsSheet.close());
  const track = (element: HTMLElement, property: string) =>
    new ResizeObserver(() => document.documentElement.style.setProperty(property, `${element.offsetHeight}px`)).observe(element);
  track(document.querySelector("header")!, "--chrome-top");
  track($("bottom-bar"), "--chrome-bottom");
}
Object.defineProperty(window, "ruiPerformance", { get: () => documentView?.diagnostics() });
let renderedState: State | undefined;
function editable(node: Semantic) {
  return !!client?.hasControl && !staging && node.states.get("Disabled") !== "True";
}
function enabled(node: Semantic) {
  return (
    !!client?.hasControl &&
    !client.pendingCommands &&
    !staging &&
    node.states.get("Disabled") !== "True"
  );
}
const ariaRole: Record<string, string> = {
  list: "list",
  item: "listitem",
  table: "table",
  row: "row",
  cell: "cell",
  dialog: "region",
};
const rowShape = (kind: string) =>
  kind.replace(/^view:(app|window)-active$/, "view:$1");
function text(element: HTMLElement, value: string) {
  if (element.textContent !== value) element.textContent = value;
}
function attribute(element: HTMLElement, name: string, value: string) {
  if (element.getAttribute(name) !== value) element.setAttribute(name, value);
}
function createRow(node: Semantic): Row {
  const group = node.role === "Group" || node.role === "Root",
    kind = node.browser_input_type ?? "";
  const element = document.createElement(
    kind === "view:disclosure" ? "details" : group ? "section" : "div",
  );
  element.className = "node";
  element.dataset.id = String(node.id);
  element.dataset.presentation = kind.startsWith("view:") ? kind.slice(5) : "";
  element.dataset.role = node.role;
  const structure = element.dataset.presentation;
  if (ariaRole[structure]) element.setAttribute("role", ariaRole[structure]);
  if (structure === "dialog")
    element.setAttribute("aria-label", "Active dialog");
  const heading = /^view:heading([1-6])$/.exec(kind);
  const label = document.createElement(
    kind === "view:disclosure"
      ? "summary"
      : heading
        ? `h${heading[1]}`
        : group
          ? "h2"
          : node.role === "TextField" || node.role === "Checkbox"
            ? "label"
            : "p",
  );
  label.textContent = node.label ?? node.role;
  const description = document.createElement("p");
  description.className = "description";
  const row: Row = { element, label, description, node, kind };
  element.append(label, description);
  if (group) {
    row.children = document.createElement("div");
    row.children.className = "children";
    if (kind === "view:apps") {
      const filter = document.createElement("input");
      filter.type = "search";
      filter.placeholder = "Find an app";
      filter.setAttribute("aria-label", "Find a shared application");
      filter.oninput = () => filterApps(row);
      element.append(filter);
      row.filter = filter;
    }
    element.append(row.children);
  } else if (node.role === "TextField") {
    const input =
      kind === "textarea"
        ? document.createElement("textarea")
        : document.createElement("input");
    if (input instanceof HTMLInputElement)
      input.type = [
        "password",
        "email",
        "url",
        "search",
        "tel",
        "number",
        "date",
        "datetime-local",
        "time",
        "month",
        "week",
      ].includes(kind)
        ? kind
        : "text";
    input.id = `field-${node.id}`;
    if (input instanceof HTMLInputElement && kind === "number") {
      input.step = "any";
      input.inputMode = "decimal";
    }
    label.setAttribute("for", input.id);
    input.autocomplete = kind === "password" ? "current-password" : "off";
    input.dir = "auto";
    input.addEventListener("input", () => {
      row.dirty ??= { version: row.node.version, authority };
      input.dataset.dirty = "true";
    });
    if (input instanceof HTMLInputElement)
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter" && !event.isComposing && !apply.disabled) {
          event.preventDefault();
          apply.click();
        }
      });
    const actions = document.createElement("div");
    actions.className = "edit-actions";
    const apply = document.createElement("button");
    apply.textContent =
      kind === "url" && node.label === "Address" ? "Go" : "Apply";
    const discard = document.createElement("button");
    discard.textContent = "Discard";
    const message = document.createElement("p");
    message.className = "message";
    discard.onclick = () => {
      row.dirty = undefined;
      input.value = sourceValue(row.node);
      input.dataset.dirty = "false";
      message.textContent = "";
    };
    apply.onclick = async () => {
      try {
        if (!enabled(row.node) || !client)
          throw Error("Take control before editing.");
        const stamp = row.dirty ?? { version: row.node.version, authority };
        if (stamp.version !== row.node.version || stamp.authority !== authority)
          throw Error(
            "The source changed. Discard this draft before editing again.",
          );
        let value = input.value;
        if (kind === "password") {
          input.value = "";
          row.dirty = undefined;
          staging = true;
          refresh();
          const staged = await api(
            "secret",
            { target: Number(node.id), secret: value },
            true,
          );
          value = staged.token;
          staging = false;
          if (
            stamp.version !== row.node.version ||
            stamp.authority !== authority ||
            !enabled(row.node)
          )
            throw Error("Control or source changed. Enter the password again.");
        }
        row.submitted = client.setBrowserValue(node.id, value);
        message.textContent = "Sending to host…";
        if (kind === "url") input.blur();
      } catch (error) {
        message.textContent = String(error);
      } finally {
        staging = false;
        refresh();
      }
    };
    actions.append(apply, discard);
    element.append(input, actions, message);
    Object.assign(row, { input, apply, message });
  } else if (node.role === "Checkbox") {
    const input = document.createElement("input");
    input.type = "checkbox";
    input.id = `field-${node.id}`;
    label.setAttribute("for", input.id);
    element.prepend(input);
    input.onchange = () => {
      const checked = input.checked;
      input.checked = !!(
        row.node.value &&
        "Boolean" in row.node.value &&
        row.node.value.Boolean
      );
      try {
        client?.setChecked(node.id, checked);
        refresh();
      } catch (error) {
        say(error);
      }
    };
    row.input = input;
  } else if (
    node.role === "Button" ||
    node.actions.some((a) => a.id === "Activate")
  ) {
    const button = document.createElement("button");
    button.dataset.kind = kind;
    button.onclick = () => {
      try {
        client?.activate(node.id);
        refresh();
      } catch (error) {
        say(error);
      }
    };
    element.prepend(button);
    label.hidden = true;
    row.button = button;
  }
  return row;
}
function filterApps(row: Row) {
  if (!row.filter || !row.children) return;
  const query = row.filter.value.trim().toLocaleLowerCase();
  for (const child of Array.from(row.children.children))
    (child as HTMLElement).hidden =
      !!query && !child.textContent?.toLocaleLowerCase().includes(query);
}
function sourceValue(node: Semantic) {
  return node.browser_input_type === "password"
    ? ""
    : node.value && "Text" in node.value
      ? node.value.Text
      : "";
}
function reset() {
  documentView?.stop();
  documentCommands = [];
  documentTarget = undefined;
  renderedState = undefined;
  authority++;
  for (const row of rows.values()) if (row.input) row.input.value = "";
  rows.clear();
  view.replaceChildren();
  for (const slot of Object.values(slots)) slot.replaceChildren();
}
function refreshControls() {
  for (const row of rows.values()) {
    const node = row.node;
    if (row.button)
      row.button.disabled =
        !enabled(node) || !node.actions.some((a) => a.id === "Activate");
    if (row.input instanceof HTMLInputElement && row.input.type === "checkbox")
      row.input.disabled = !enabled(node);
    else if (row.input)
      row.input.readOnly =
        !editable(node) || row.submitted !== undefined || !node.actions.some((a) => a.id === "SetValue");
    if (row.apply)
      row.apply.disabled =
        !enabled(node) || !node.actions.some((a) => a.id === "SetValue");
  }
}
function refresh() {
  if (renderedState) render(renderedState);
}
function render(state: State) {
  renderedState = state;
  documentTarget = undefined;
  const nodes = new Map<bigint, Semantic>();
  for (const record of state.records.values())
    if ("Semantic" in record) nodes.set(record.Semantic.id, record.Semantic);
  const parents = new Map<bigint, Semantic>();
  for (const node of nodes.values())
    for (const child of node.children) parents.set(child.id, node);
  for (const [id, row] of rows)
    if (!nodes.has(id)) {
      row.element.remove();
      rows.delete(id);
    }
  for (const node of nodes.values()) {
    let row = rows.get(node.id);
    if (
      row &&
      (row.node.role !== node.role ||
        rowShape(row.kind) !== rowShape(node.browser_input_type ?? ""))
    ) {
      row.element.remove();
      rows.delete(node.id);
      row = undefined;
    }
    if (!row) {
      row = createRow(node);
      rows.set(node.id, row);
    }
    row.node = node;
    const kind = node.browser_input_type ?? "";
    if (kind === "document-command") {
      row.element.hidden = true;
      documentTarget = node;
    }
    if (row.kind !== kind) {
      row.kind = kind;
      row.element.dataset.presentation = kind.startsWith("view:")
        ? kind.slice(5)
        : "";
      if (row.button) row.button.dataset.kind = kind;
    }
    text(row.label, node.label ?? node.role);
    if (!row.button) row.label.hidden = !node.label;
    if (row.filter) {
      row.filter.hidden = node.children.length < 5;
      if (row.filter.hidden) row.filter.value = "";
    }
    text(row.description, node.description ?? "");
    row.description.hidden = !node.description;
    if (row.button) {
      let label = node.label ?? "Activate";
      attribute(row.button, "aria-label", label);
      if (
        workspace === "desktop" &&
        /^(Select|Selected|Deselect|Scroll to): /.test(node.label ?? "")
      ) {
        label = node.label!.startsWith("Scroll to:")
          ? "Scroll into view"
          : node.label!.split(":")[0];
      }
      if (workspace === "browser" && parents.get(node.id)?.browser_input_type === "view:tabs") {
        const current = label.startsWith("Current: ");
        if (current) label = label.slice("Current: ".length);
        row.element.dataset.current = String(current);
      }
      text(row.button, label);
      if (["view:app", "view:window"].includes(rowShape(row.kind))) {
        attribute(
          row.button,
          "aria-pressed",
          String(row.kind.endsWith("-active")),
        );
      }
    }
    if (row.input) {
      if (
        row.input instanceof HTMLInputElement &&
        row.input.type === "checkbox"
      ) {
        row.input.checked = !!(
          node.value &&
          "Boolean" in node.value &&
          node.value.Boolean
        );
      } else {
        if (!row.dirty && row.input.value !== sourceValue(node))
          row.input.value = sourceValue(node);
        if (
          row.dirty?.version !== undefined &&
          row.dirty.version !== node.version &&
          row.message
        )
          text(
            row.message,
            "The source changed. Your draft has been kept; discard it to edit the new value.",
          );
      }
    }
    if (row.apply) {
      row.apply.parentElement!.hidden = !node.actions.some(
        (a) => a.id === "SetValue",
      );
    }
  }
  refreshControls();
  const cursors = new Map<HTMLElement, Element | null>();
  const place = (container: HTMLElement, ids: bigint[], parent?: Row) => {
    cursors.set(container, container.firstElementChild);
    for (const id of ids) {
      const row = rows.get(id);
      if (!row) continue;
      const target = chromeSlot(row, parent) ?? container;
      if (!cursors.has(target)) cursors.set(target, target.firstElementChild);
      const cursor = cursors.get(target)!;
      if (row.element !== cursor) target.insertBefore(row.element, cursor);
      cursors.set(target, row.element.nextElementSibling);
      if (row.children)
        place(
          row.children,
          row.node.children.map((r) => r.id),
          row,
        );
    }
  };
  const owned = new Set(
    [...nodes.values()].flatMap((n) => n.children.map((r) => r.id)),
  );
  place(
    view,
    [...nodes.keys()].filter((id) => !owned.has(id)),
  );
  for (const row of rows.values()) filterApps(row);
  if (workspace === "browser")
    $("tabs-count").textContent = String(
      Array.from(slots.tabs.querySelectorAll("button")).filter((b) => b.getAttribute("aria-label") !== "Close tab").length,
    );
  const surfaces = [...state.records.values()].flatMap((r) =>
    "Surface" in r ? [r.Surface] : [],
  );
  $("coverage").textContent = surfaces
    .flatMap((s) => s.unsupported_features)
    .join(" ");
}
const icons: Record<string, string> = { Back: "back", Forward: "forward", Reload: "reload" };
function chromeSlot(row: Row, parent?: Row): HTMLElement | undefined {
  if (workspace !== "browser") return;
  const label = row.node.label ?? "";
  if (parent?.kind === "view:toolbar") {
    if (row.input) return slots.address;
    row.element.dataset.icon = icons[label] ?? "";
    return label === "Reload" ? slots.reload : slots.nav;
  }
  if (row.kind === "view:toolbar") return slots.store;
  if (row.kind === "view:tabs") return slots.tabs;
  if (parent?.kind === "view:disclosure" && label === "New tab") {
    row.element.dataset.icon = "plus";
    return slots.newTab;
  }
  if (row.kind === "view:disclosure" && parent?.node.role === "Root") {
    (row.element as HTMLDetailsElement).open = true;
    return slots.menu;
  }
  if (row.kind === "view:dialog" || (row.children && label === "Choose a host file")) return slots.prompt;
  if (row.children && label === "Host downloads") return slots.menu;
}
function buttons() {
  control.disabled =
    !connected ||
    !client ||
    client.hasControl ||
    !client.capabilities.can_control;
  release.disabled = !client?.hasControl;
  control.hidden = !!client?.hasControl || !connected;
  release.hidden = !client?.hasControl;
  connect.hidden = connected || !device;
  document.body.dataset.connected = String(connected);
  document.body.dataset.control = String(!!client?.hasControl);
  retry.hidden = !client?.pendingCommands;
  retry.textContent = uncertainAction ? "Check host, then reconnect" : "Check action status";
  connection.textContent = client?.hasControl
    ? "Controlling host"
    : connected
      ? "Viewing host"
      : device
        ? "Disconnected"
        : "Not paired";
}
async function connectHost() {
  if (!device || connecting) return;
  connecting = true;
  connect.disabled = true;
  try {
    if (!client) {
      client = await BrowserClient.create({
        mode: "semanticOnly",
        selection: workspace,
        url: websocketURL.href,
        allowInsecure: location.protocol === "http:",
        clientId: bytes(device.client),
        credential: async () => bytes(device!.credential),
      });
      client.subscribe((event) => {
        if (event.type === "connected") connected = true;
        if (event.type === "applied") {
          try {
            const state = client!.state!;
            render(state);
            client!.semanticReady(state.revision);
            documentView?.start();
          } catch (error) {
            say(error);
          }
        }
        if (event.type === "semanticReady") { refreshControls(); queueMicrotask(drainDocumentCommands); }
        if (event.type === "control") {
          if (!event.granted) {
            documentCommands = [];
            authority++;
            for (const row of rows.values())
              if (row.kind === "password" && row.input) row.input.value = "";
          }
          refresh();
        }
        if (event.type === "commandResult") {
          uncertainAction = ["outcomeunknown", "outcomeexpired"].includes(event.status.toLowerCase());
          if (uncertainAction) documentCommands = [];
          queueMicrotask(drainDocumentCommands);
          say(
            uncertainAction
              ? "The host could not confirm this action. Check the host page, then reconnect to continue. Reconnect will not repeat the action."
              : event.status.toLowerCase() === "dispatched"
              ? "Action sent to the host. Content updates separately."
              : `${event.status}${event.rejection ? `: ${event.rejection}` : ""}`,
          );
          for (const row of rows.values())
            if (row.submitted === event.sequence) {
              row.submitted = undefined;
              if (event.status.toLowerCase() === "dispatched") {
                row.dirty = undefined;
                if (row.input) row.input.dataset.dirty = "false";
                if (row.message) row.message.textContent = "Sent to host";
              } else if (row.message) row.message.textContent = event.status;
            }
          refresh();
        }
        if (event.type === "disconnected") {
          connected = false;
          reset();
          connect.disabled = false;
          connection.textContent = "Disconnected";
          say("Connection lost. Reconnect, then take control again.");
        }
        if (event.type === "reset") reset();
        if (event.type === "failure") say(event.message);
        buttons();
      });
    }
    // A reload can reach the host before its previous socket has finished
    // cleanup. Retry only this authenticated busy response, as the SDK's
    // consumer does; credential and other connection failures remain explicit.
    const current = client;
    const deadline = performance.now() + 2000;
    for (;;) {
      try {
        await current.connect();
        break;
      } catch (error) {
        if (
          !(error instanceof Error) ||
          error.message !==
            "host is still closing the previous connection; reconnect shortly" ||
          performance.now() >= deadline ||
          current !== client
        )
          throw error;
        await new Promise((done) => setTimeout(done, 50));
      }
    }
    connect.disabled = true;
    say("Connected. Take control to interact with the host.");
    buttons();
  } catch (error) {
    say(error);
    connect.disabled = false;
  } finally {
    connecting = false;
  }
}
connect.onclick = () => void connectHost();
control.onclick = () => {
  try {
    if (client && renderedState) client.semanticReady(renderedState.revision);
    client?.requestControl();
  } catch (error) {
    say(error);
  }
};
release.onclick = () => client?.releaseControl();
retry.onclick = () => {
  try {
    if (uncertainAction) {
      client?.dispose();
      location.reload();
      return;
    }
    client?.retryPending();
  } catch (error) {
    say(error);
  }
};
$("forget").onclick = async () => {
  client?.dispose();
  client = undefined;
  device = undefined;
  reset();
  await storage(null);
  connection.textContent = "Not paired";
  say("Device forgotten here. Use revoke on the host to revoke its approval.");
  connect.disabled = false;
  buttons();
};
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") {
    documentView?.stop();
    documentCommands = [];
    authority++;
    for (const row of rows.values())
      if (row.kind === "password" && row.input) row.input.value = "";
  } else if (client && renderedState) {
    refresh();
    try {
      client.semanticReady(renderedState.revision);
      documentView?.start();
    } catch {}
  }
});
window.addEventListener("pagehide", () => client?.disconnect());
device = await storage();
buttons();
const invitation = new URLSearchParams(location.hash.slice(1)).get("invite");
history.replaceState(null, "", location.pathname);
if (!device && invitation) {
  $("pairing").hidden = false;
  const clientId = hex(crypto.getRandomValues(new Uint8Array(16)));
  try {
    const pending = await api("pair", { invitation, client: clientId });
    $("pair-status").textContent =
      `On the host, approve this code: ${pending.code}`;
    const deadline = Date.now() + 120000;
    while (Date.now() < deadline && !device) {
      await new Promise((r) => setTimeout(r, 1000));
      const result = await api("pair/poll", {
        client: clientId,
        ticket: pending.ticket,
      });
      if (result.credential) {
        device = { client: clientId, credential: result.credential };
        await storage(device);
        $("pairing").hidden = true;
      }
    }
    if (!device)
      throw Error("Pairing expired. Request a fresh invitation from the host.");
  } catch (error) {
    say(error);
  }
}
if (device) await connectHost();
