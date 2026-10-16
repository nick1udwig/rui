// Adapt the owned browser's guarded semantics to the desktop's source contract.
// Public desktop IDs are remapped; browser handles and secret bindings stay local.
import { Workspace } from "../browser/workspace.mjs";
import { resolve } from "node:path";

export class DesktopBrowser {
  static async launch(directory) {
    return new DesktopBrowser(
      await Workspace.launch(resolve(directory, "browser")),
    );
  }
  constructor(workspace) {
    this.workspace = workspace;
    this.pending = Promise.resolve();
    this.source = {
      root: ["rui-browser", "managed"],
      pid: process.pid,
      start: String(process.pid),
      name: "Web browser",
      adapter: "browser",
    };
  }
  request(request) {
    const result = this.pending.then(() => this.run(request));
    this.pending = result.catch(() => {});
    return result;
  }
  async run(request) {
    if (request.op === "capture") {
      const capture = await this.workspace.capture();
      const parents = new Map(
        capture.nodes.flatMap((n) => n.children.map((id) => [id, n.id])),
      );
      const key = (id) => ["rui-browser", String(id)];
      return {
        status: capture.status,
        nodes: capture.nodes.map((node) => ({
          ...node,
          key: key(node.id),
          parent: parents.has(node.id) ? key(parents.get(node.id)) : null,
          children: node.children.map(key),
          role: node.role === "root" ? "group" : node.role,
          fingerprint: String(node.version),
        })),
      };
    }
    if (request.op === "dispatch") {
      const result = await this.workspace.dispatch({
        ...request,
        target: Number(request.key[1]),
        version: Number(request.fingerprint),
        deadlineMs: Math.max(0, request.expires - Date.now()),
      });
      try {
        result.capture = await this.run({ op: "capture" });
      } catch {}
      return result;
    }
    if (request.op === "diagnose") {
      const { nodes } = await this.run({ op: "capture" });
      return {
        status: "readable",
        counts: {
          windows: this.workspace.pages.size,
          text_fields: nodes.filter((n) => n.role === "text").length,
          editable_text: nodes.filter((n) => n.actions.includes("SetValue"))
            .length,
          numeric_controls: nodes.filter((n) => n.kind === "number").length,
          selection_controls: 0,
          actions: nodes.filter((n) => n.actions.includes("Activate")).length,
          protected_fields: nodes.filter((n) => n.kind === "password").length,
          unnamed_controls: 0,
        },
        scope:
          "Managed host browser. Responsive structured content; site CSS, canvas and video are unavailable.",
      };
    }
    throw Error("Unsupported browser operation");
  }
  stage(scope, secret, target) {
    return this.workspace.stage(
      {
        ...scope,
        target: Number(target.key[1]),
        version: Number(target.fingerprint),
      },
      secret,
    );
  }
  clearSecrets() {
    this.workspace.secrets.clear();
  }
  async close() {
    await this.pending;
    return this.workspace.context.close();
  }
}
