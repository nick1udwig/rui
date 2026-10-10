import { BrowserClient, SemanticView } from "@rui/client-browser";
import "./style.css";
const view = new SemanticView(document.getElementById("view")!);
const file = document.getElementById("credential") as HTMLInputElement;
const connect = document.getElementById("connect") as HTMLButtonElement;
const disconnect = document.getElementById("disconnect") as HTMLButtonElement;
const status = document.getElementById("status")!;
let client: BrowserClient | undefined;
connect.addEventListener("click", async () => {
  if (!file.files?.[0]) {
    status.textContent = "Choose an approved credential file.";
    return;
  }
  connect.disabled = true;
  try {
    client?.dispose();
    view.reset();
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
    client.subscribe((event) => {
      if (event.type === "applied" || event.type === "resumed") {
        const state = client?.state;
        if (!state) return;
        view.render(state);
        client!.semanticReady(state.revision);
        status.textContent = `Observer · revision ${state.revision}`;
      } else if (event.type === "reset" || event.type === "disconnected") {
        view.reset();
        status.textContent = "Disconnected";
      } else if (event.type === "failure" || event.type === "error")
        status.textContent =
          event.type === "failure" ? event.message : event.code;
    });
    await client.connect();
    disconnect.disabled = false;
  } catch (error) {
    status.textContent = String(error);
    connect.disabled = false;
  }
});
disconnect.addEventListener("click", () => {
  client?.disconnect();
  disconnect.disabled = true;
  connect.disabled = false;
});
window.addEventListener("pagehide", () => {
  client?.disconnect();
  view.reset();
  connect.disabled = false;
  disconnect.disabled = true;
});
