import {
  BrowserClient,
  bindPointer,
  semantic,
  text,
  type EditCapture,
  type Id,
} from "@rui/client-browser";
import "./style.css";
const app = document.querySelector<HTMLDivElement>("#app")!;
app.innerHTML = `<main><header><div><p class="eyebrow">RUI · browser client</p><h1>The host owns this view.</h1><p>Watch the GTK host change here. Take control to edit it from your browser.</p></div><span id="connection" class="badge">Disconnected</span></header><section class="connect"><label>Approved credential <input id="credential" type="file"></label><button id="connect">Connect</button><button id="disconnect" disabled>Disconnect</button><button id="control" disabled>Take control</button><button id="release" disabled>Release control</button><button id="recover" hidden>Resolve pending command</button></section><p id="status" role="status">Choose the client.credential file created by the host, then connect.</p><section class="mirror"><div class="mirror-head"><span>Host’s published canvas</span><span id="revision">Waiting for a view</span></div><div class="viewport"><canvas id="mirror" aria-label="Interactive host mirror"></canvas><p id="empty">Your connected host will appear here.</p></div></section><section id="editor" hidden><div><h2 id="field">Remote text field</h2><p>Edits stay in this draft until you apply them to the host. Ctrl + Enter applies.</p></div><textarea id="draft" aria-label="Remote text draft" rows="2" maxlength="1024"></textarea><button id="apply" disabled>Apply to host</button><p id="edit-status" role="status"></p></section><footer><span>Click a button in the mirror. Click a text field to open its editor below.</span><span id="timing"></span></footer></main>`;
const element = <T extends HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const canvas = element<HTMLCanvasElement>("mirror"),
  credential = element<HTMLInputElement>("credential");
const status = element("status"),
  connect = element<HTMLButtonElement>("connect"),
  disconnect = element<HTMLButtonElement>("disconnect"),
  control = element<HTMLButtonElement>("control"),
  release = element<HTMLButtonElement>("release"),
  recover = element<HTMLButtonElement>("recover");
const draft = element<HTMLTextAreaElement>("draft"),
  apply = element<HTMLButtonElement>("apply"),
  editStatus = element("edit-status");
let file: File | undefined,
  client: BrowserClient | undefined,
  unbind: (() => void) | undefined,
  target: Id | undefined,
  capture: EditCapture | undefined,
  dirty = false,
  pending: bigint | undefined;
function clearEditor() {
  target = undefined;
  capture = undefined;
  dirty = false;
  pending = undefined;
  element("editor").hidden = true;
  draft.value = "";
  apply.disabled = true;
}
function refreshEditor() {
  if (!client || target === undefined) return;
  const state = client.state,
    n = state && semantic(state, target),
    model = state && text(state, target);
  if (!n || !model) {
    clearEditor();
    return;
  }
  element("field").textContent = n.label ?? "Remote text field";
  if (dirty && capture && model.text_version !== capture.textVersion) {
    dirty = false;
    capture = undefined;
    editStatus.textContent = "The host changed this field. Start a fresh edit.";
  }
  if (!dirty && !pending) {
    draft.value = model.content ?? "";
    try {
      capture = client.captureEdit(target);
    } catch {
      capture = undefined;
    }
  }
  apply.disabled =
    !dirty || !capture || pending !== undefined || !client.hasControl;
  draft.disabled = !capture || pending !== undefined || !client.hasControl;
}
credential.onchange = () => {
  file = credential.files?.[0];
};
connect.onclick = async () => {
  if (!file) {
    status.textContent = "Choose the host’s client.credential file first.";
    return;
  }
  connect.disabled = true;
  clearEditor();
  try {
    if (!client) {
      client = await BrowserClient.create({
        url: `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/rui`,
        allowInsecure: location.protocol === "http:",
        clientId: new Uint8Array(16).fill(3),
        selection: "fixture",
        canvas,
        credential: async () => {
          if (!file || file.size !== 32)
            throw new Error("expected the 32-byte client.credential file");
          return new Uint8Array(await file.arrayBuffer());
        },
        onEvent: (event) => {
          recover.hidden = !client?.pendingCommands;
          if (event.type === "connected") {
            element("connection").textContent = "Observer";
            status.textContent =
              "Connected. The host owns the state; take control to edit.";
            disconnect.disabled = false;
            control.disabled = !event.can_control;
          }
          if (event.type === "control") {
            if (!event.granted) clearEditor();
            element("connection").textContent = event.granted
              ? "Controller"
              : "Observer";
            release.disabled = !event.granted;
            control.disabled = event.granted;
            refreshEditor();
          }
          if (event.type === "presented") {
            element("empty").hidden = true;
            element("revision").textContent = `Revision ${event.revision}`;
            element("timing").textContent =
              `Draw ${event.renderMs.toFixed(1)} ms`;
            refreshEditor();
          }
          if (event.type === "commandResult") {
            if (pending === event.sequence) {
              pending = undefined;
              dirty = false;
              capture = undefined;
              editStatus.textContent =
                event.status === "Applied"
                  ? "Host applied your edit."
                  : `Host result: ${event.status}${event.rejection ? ` (${event.rejection})` : ""}`;
            }
            refreshEditor();
          }
          if (event.type === "error") {
            status.textContent = `Host: ${event.code}`;
          }
          if (event.type === "failure") {
            status.textContent = event.message;
          }
          if (event.type === "disconnected") {
            element("connection").textContent = "Disconnected";
            connect.disabled = false;
            disconnect.disabled = true;
            control.disabled = true;
            release.disabled = true;
            clearEditor();
          }
        },
      });
      unbind = bindPointer(
        client,
        canvas,
        (id) => {
          target = id;
          dirty = false;
          capture = undefined;
          element("editor").hidden = false;
          editStatus.textContent = "Waiting for the host to focus this field…";
          refreshEditor();
        },
        (message) => (status.textContent = message),
      );
    }
    status.textContent = "Connecting…";
    await client.connect();
  } catch (error) {
    status.textContent = String(error);
    connect.disabled = false;
  }
};
disconnect.onclick = () => client?.disconnect();
control.onclick = () => client?.requestControl();
release.onclick = () => client?.releaseControl();
recover.onclick = () => {
  try {
    client?.retryPending();
    status.textContent = "Resolving the original command with the host…";
  } catch (error) {
    status.textContent = String(error);
  }
};
draft.oninput = () => {
  dirty = true;
  apply.disabled = !capture || pending !== undefined || !client?.hasControl;
};
apply.onclick = () => {
  if (!client || !capture || !dirty) return;
  try {
    pending = client.commitEdit(capture, draft.value);
    draft.disabled = true;
    apply.disabled = true;
    editStatus.textContent = "Waiting for the host…";
  } catch {
    dirty = false;
    capture = undefined;
    editStatus.textContent = "The view changed. Start a fresh edit.";
    refreshEditor();
  }
};
draft.onkeydown = (event) => {
  if (event.ctrlKey && event.key === "Enter") {
    event.preventDefault();
    apply.click();
  }
};
window.addEventListener("pagehide", () => {
  unbind?.();
  client?.dispose();
});
