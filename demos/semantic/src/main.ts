import { BrowserClient, SemanticControls } from "@rui/client-browser";
import "./style.css";
const root = document.getElementById("view")!,
  status = document.getElementById("status")!;
const file = document.getElementById("credential") as HTMLInputElement;
const connect = document.getElementById("connect") as HTMLButtonElement;
const disconnect = document.getElementById("disconnect") as HTMLButtonElement;
const control = document.getElementById("control") as HTMLButtonElement;
const release = document.getElementById("release") as HTMLButtonElement;
const recover = document.getElementById("recover") as HTMLButtonElement;
const connection = document.getElementById("connection")!;
let client: BrowserClient | undefined, controls: SemanticControls | undefined;
connect.addEventListener("click", async () => {
  if (!file.files?.[0]) {
    status.textContent = "Choose an approved credential file.";
    return;
  }
  connect.disabled = true;
  try {
    if (!client) {
      const credentialFile = file.files[0];
      client = await BrowserClient.create({
        mode: "semanticOnly",
        url: `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/rui`,
        allowInsecure: location.protocol === "http:",
        clientId: new Uint8Array(16).fill(3),
        selection: "fixture",
        credential: async () =>
          new Uint8Array(await credentialFile.arrayBuffer()),
      });
      controls = new SemanticControls(client, root, status);
      client.subscribe((event) => {
        if (event.type === "connected") {
          disconnect.disabled = false;
          control.disabled = !event.can_control;
          connection.textContent = "Observer";
        }
        if (event.type === "control" || event.type === "semanticReady") {
          const held = client!.hasControl;
          release.disabled = !held;
          connection.textContent = held ? "Controller" : "Observer";
        }
        if (event.type === "commandResult")
          recover.hidden = client!.pendingCommands === 0;
        if (event.type === "disconnected") {
          connect.disabled = false;
          disconnect.disabled = true;
          control.disabled = true;
          release.disabled = true;
          connection.textContent = "Disconnected";
          recover.hidden = client!.pendingCommands === 0;
        }
      });
    }
    await client.connect();
  } catch (error) {
    status.textContent = String(error);
    connect.disabled = false;
  }
});
disconnect.addEventListener("click", () => client?.disconnect());
control.addEventListener("click", () => {
  try {
    controls?.consumeCurrent();
    client?.requestControl();
  } catch (error) {
    status.textContent = String(error);
  }
});
release.addEventListener("click", () => client?.releaseControl());
recover.addEventListener("click", () => {
  try {
    client?.retryPending();
    status.textContent = "Resolving the original command bytes with the host.";
  } catch (error) {
    status.textContent = String(error);
  }
});
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible")
    try {
      controls?.consumeCurrent();
    } catch (error) {
      status.textContent = String(error);
    }
});
window.addEventListener("pagehide", () => client?.disconnect());
