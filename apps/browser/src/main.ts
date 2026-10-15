import { BrowserClient, type Semantic, type State } from "@rui/client-browser";
import "./style.css";
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
  connected = false;
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
async function api(path: string, data: unknown, authenticated = false) {
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
  });
  const result = await response.json();
  if (!response.ok) throw Error(result.error);
  return result;
}
function say(error: unknown) {
  status.textContent = String(error);
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
};
const rows = new Map<bigint, Row>();
function enabled(node: Semantic) {
  return (
    !!client?.hasControl &&
    !client.pendingCommands &&
    !staging &&
    node.states.get("Disabled") !== "True"
  );
}
function createRow(node: Semantic): Row {
  const group = node.role === "Group" || node.role === "Root",
    kind = node.browser_input_type ?? "";
  const element = document.createElement(group ? "section" : "div");
  element.className = "node";
  element.dataset.id = String(node.id);
  const label = document.createElement(group ? "h2" : "label");
  label.textContent = node.label ?? node.role;
  const description = document.createElement("p");
  description.className = "description";
  const row: Row = { element, label, description, node, kind };
  element.append(label, description);
  if (group) {
    row.children = document.createElement("div");
    row.children.className = "children";
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
    label.setAttribute("for", input.id);
    input.autocomplete = kind === "password" ? "current-password" : "off";
    input.dir = "auto";
    input.addEventListener("input", () => {
      row.dirty ??= { version: row.node.version, authority };
      input.dataset.dirty = "true";
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
          if (client.state) render(client.state);
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
      } catch (error) {
        message.textContent = String(error);
      } finally {
        staging = false;
        if (client?.state) render(client.state);
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
      } catch (error) {
        say(error);
      }
    };
    row.input = input;
  } else if (node.actions.some((a) => a.id === "Activate")) {
    const button = document.createElement("button");
    button.dataset.kind = kind;
    button.onclick = () => {
      try {
        client?.activate(node.id);
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
function sourceValue(node: Semantic) {
  return node.browser_input_type === "password"
    ? ""
    : node.value && "Text" in node.value
      ? node.value.Text
      : "";
}
function reset() {
  authority++;
  for (const row of rows.values()) if (row.input) row.input.value = "";
  rows.clear();
  view.replaceChildren();
}
function render(state: State) {
  const nodes = new Map<bigint, Semantic>();
  for (const record of state.records.values())
    if ("Semantic" in record) nodes.set(record.Semantic.id, record.Semantic);
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
        row.kind !== (node.browser_input_type ?? ""))
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
    row.label.textContent = node.label ?? node.role;
    row.description.textContent = node.description ?? "";
    row.description.hidden = !node.description;
    if (row.button) {
      row.button.textContent = node.label ?? "Activate";
      row.button.disabled = !enabled(node);
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
        row.input.disabled = !enabled(node);
      } else {
        row.input.readOnly = !enabled(node);
        if (!row.dirty) row.input.value = sourceValue(node);
        if (
          row.dirty?.version !== undefined &&
          row.dirty.version !== node.version &&
          row.message
        )
          row.message.textContent =
            "The source changed. Your draft has been kept; discard it to edit the new value.";
      }
    }
    if (row.apply) row.apply.disabled = !enabled(node);
  }
  const place = (container: HTMLElement, ids: bigint[]) => {
    let cursor = container.firstElementChild;
    for (const id of ids) {
      const row = rows.get(id);
      if (!row) continue;
      if (row.element !== cursor) container.insertBefore(row.element, cursor);
      cursor = row.element.nextElementSibling;
      if (row.children)
        place(
          row.children,
          row.node.children.map((r) => r.id),
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
  const surfaces = [...state.records.values()].flatMap((r) =>
    "Surface" in r ? [r.Surface] : [],
  );
  $("coverage").textContent = surfaces
    .flatMap((s) => s.unsupported_features)
    .join(" ");
}
function buttons() {
  control.disabled =
    !connected ||
    !client ||
    client.hasControl ||
    !client.capabilities.can_control;
  release.disabled = !client?.hasControl;
  retry.hidden = !client?.pendingCommands;
  connection.textContent = client?.hasControl
    ? "Controlling host"
    : connected
      ? "Viewing host"
      : "Disconnected";
}
async function connectHost() {
  if (!device || connecting) return;
  connecting = true;
  connect.disabled = true;
  try {
    if (!client) {
      client = await BrowserClient.create({
        mode: "semanticOnly",
        selection: "browser",
        url: websocketURL.href,
        allowInsecure: location.protocol === "http:",
        clientId: bytes(device.client),
        credential: async () => bytes(device!.credential),
      });
      client.subscribe((event) => {
        if (event.type === "connected") connected = true;
        if (event.type === "applied") {
          try {
            render(client!.state!);
            client!.semanticReady(client!.state!.revision);
          } catch (error) {
            say(error);
          }
        }
        if (event.type === "semanticReady" && client?.state)
          render(client.state);
        if (event.type === "control") {
          if (!event.granted) {
            authority++;
            for (const row of rows.values())
              if (row.kind === "password" && row.input) row.input.value = "";
          }
          if (client?.state) render(client.state);
        }
        if (event.type === "commandResult") {
          say(
            event.status.toLowerCase() === "dispatched"
              ? "Action sent to the host browser. Page state updates separately."
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
          if (client?.state) render(client.state);
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
    await client.connect();
    say("Connected. Take control to interact with the host browser.");
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
    if (client?.state) client.semanticReady(client.state.revision);
    client?.requestControl();
  } catch (error) {
    say(error);
  }
};
release.onclick = () => client?.releaseControl();
retry.onclick = () => {
  try {
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
    authority++;
    for (const row of rows.values())
      if (row.kind === "password" && row.input) row.input.value = "";
  } else if (client?.state) {
    render(client.state);
    try {
      client.semanticReady(client.state.revision);
    } catch {}
  }
});
window.addEventListener("pagehide", () => client?.disconnect());
device = await storage();
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
