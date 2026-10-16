import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./native.py", import.meta.url));
export function native(request) {
  return new Promise((resolve, reject) => {
    const child = execFile(
      "/usr/bin/python3",
      [script],
      {
        timeout: 2400,
        maxBuffer: 1024 * 1024,
        encoding: "utf8",
      },
      (error, stdout) => {
        if (error)
          return reject(Error("Accessibility worker unavailable or timed out"));
        try {
          const result = JSON.parse(stdout);
          if (result.error) throw Error(result.error);
          resolve(result);
        } catch (error) {
          reject(error);
        }
      },
    );
    child.stdin.on("error", () => {});
    child.stdin.end(JSON.stringify(request));
  });
}

export class DesktopWorkspace {
  static async launch() {
    const workspace = new DesktopWorkspace();
    // Fail startup with a useful error when not launched in a desktop session.
    await workspace.local({ op: "apps" });
    workspace.timer = setInterval(() => void workspace.refresh(), 500);
    return workspace;
  }
  constructor(invoke = native) {
    this.invoke = invoke;
    this.catalog = new Map();
    this.approved = new Map();
    this.selected = undefined;
    this.generation = 0;
    this.next = 10;
    this.keys = new Map();
    this.versions = new Map();
    this.targets = new Map();
    this.sample = undefined;
    this.busy = undefined;
    this.closed = false;
    this.availability =
      "No applications shared. Use apps, then share ID in the host terminal.";
    this.context = {
      close: async () => {
        this.closed = true;
        clearInterval(this.timer);
        await this.busy;
      },
    };
    this.rebuild();
  }
  async local(request) {
    if (request.op === "apps") {
      const result = await this.invoke({ op: "list" });
      if (!Array.isArray(result.apps) || result.apps.length > 64)
        throw Error("Application limit");
      this.catalog = new Map(
        result.apps.map((source) => {
          const id = createHash("sha256")
            .update(JSON.stringify([source.root, source.pid, source.start]))
            .digest("hex")
            .slice(0, 16);
          return [id, source];
        }),
      );
      return {
        apps: [...this.catalog].map(([id, source]) => ({
          id,
          name: source.name,
          pid: source.pid,
          shared: this.approved.has(id),
        })),
      };
    }
    if (request.op === "share") {
      const source = this.catalog.get(request.id);
      if (!source)
        throw Error("Run apps and use an application ID from that list");
      if (this.approved.size >= 8 && !this.approved.has(request.id))
        throw Error("Eight shared applications maximum");
      const sample = await this.invoke({ op: "capture", source });
      this.approved.set(request.id, source);
      this.select(request.id);
      this.sample = sample;
      this.availability =
        "Observed accessibility content. Graphics and password fields are unavailable.";
      this.rebuild();
      return { shared: source.name, id: request.id };
    }
    if (request.op === "unshare") {
      if (!this.approved.delete(request.id))
        throw Error("Application is not shared");
      if (this.selected === request.id)
        this.select(this.approved.keys().next().value);
      else {
        this.generation++;
        this.sample = undefined;
      }
      this.rebuild();
      return { unshared: request.id };
    }
    throw Error("Unsupported desktop host command");
  }
  select(id) {
    this.selected = id;
    this.generation++;
    this.sample = undefined;
    this.availability = id
      ? "Reading application…"
      : "No applications shared. Use apps, then share ID in the host terminal.";
    this.rebuild();
  }
  async refresh() {
    if (this.closed || this.busy || !this.selected) return;
    const id = this.selected,
      generation = this.generation,
      source = this.approved.get(id);
    this.busy = (async () => {
      try {
        const sample = await this.invoke({ op: "capture", source });
        if (this.generation !== generation || this.closed) return;
        this.sample = sample;
        this.availability =
          "Observed accessibility content. Graphics and password fields are unavailable.";
      } catch {
        if (this.generation !== generation || this.closed) return;
        // Clear content and targets, rather than leaving stale editable controls.
        this.sample = undefined;
        this.availability =
          "Application unavailable, closed, or too large to read. Retrying; no controls are active.";
      }
      this.rebuild();
    })();
    try {
      await this.busy;
    } finally {
      this.busy = undefined;
    }
  }
  rebuild() {
    const keys = new Map(),
      versions = new Map(),
      targets = new Map(),
      nodes = [];
    const idFor = (key) => {
      if (!keys.has(key)) {
        if (this.next >= 1_000_000)
          throw Error("Desktop identity limit; restart the host");
        keys.set(key, this.keys.get(key) ?? this.next++);
      }
      return keys.get(key);
    };
    const add = (key, data, handler, guard = "") => {
      const id = key === "root" ? 3 : idFor(key);
      const node = {
        id,
        version: 1,
        role: "label",
        label: "",
        description: "",
        children: [],
        actions: [],
        disabled: false,
        ...data,
      };
      const signature = JSON.stringify([node, guard]);
      const prior = this.versions.get(id);
      node.version = prior
        ? prior.version + Number(prior.signature !== signature)
        : 1;
      versions.set(id, { signature, version: node.version });
      if (handler) targets.set(id, { ...handler, node });
      nodes.push(node);
      return id;
    };
    const apps = [];
    for (const [id, source] of this.approved) {
      apps.push(
        add(
          `app:${id}`,
          {
            role: "button",
            label: `${id === this.selected ? "Viewing: " : "Open: "}${source.name}`,
            actions: ["Activate"],
          },
          { select: id },
        ),
      );
    }
    const children = [
      add("apps", {
        role: "group",
        label: "Shared applications",
        children: apps,
      }),
    ];
    if (this.sample && this.selected) {
      const source = this.approved.get(this.selected);
      const prefix = `${this.generation}:${this.selected}:`;
      const nodeKey = (key) => prefix + JSON.stringify(key);
      // Drop layout-only containers and duplicated button labels from the
      // mobile presentation. Dispatch still validates the full native tree.
      const byKey = new Map(
        this.sample.nodes.map((n) => [JSON.stringify(n.key), n]),
      );
      const visible = [];
      const present = (raw, appRoot = false) => {
        const childKeys = raw.children.flatMap((key) => {
          const child = byKey.get(JSON.stringify(key));
          if (
            raw.role === "button" &&
            child.role === "label" &&
            child.label === raw.label &&
            !child.children.length &&
            !child.actions.length
          )
            return [];
          return present(child);
        });
        if (
          appRoot ||
          (raw.role === "group" && !raw.label && !raw.actions.length)
        )
          return childKeys;
        visible.push({ ...raw, children: childKeys });
        return [raw.key];
      };
      const roots = present(this.sample.nodes[0], true);
      for (const raw of visible) {
        const key = nodeKey(raw.key),
          structural = raw.children.length > 0;
        const node = {
          role: raw.role,
          label:
            raw.label ||
            (raw.role === "text"
              ? "Text"
              : raw.role === "button"
                ? "Activate"
                : ""),
          description: raw.description || "",
          actions: raw.actions,
          disabled: raw.disabled,
          ...(raw.kind ? { kind: raw.kind, value: raw.value } : {}),
          children: raw.children.map((k) => idFor(nodeKey(k))),
        };
        const handler = raw.actions.length
          ? { source, key: raw.key, fingerprint: raw.fingerprint }
          : undefined;
        if (structural && (raw.kind || raw.actions.length)) {
          const content = add(
            key + ":control",
            { ...node, children: [] },
            handler,
            raw.fingerprint,
          );
          add(key, {
            role: "group",
            label: raw.label,
            children: [content, ...node.children],
          });
        } else {
          add(key, node, handler, handler ? raw.fingerprint : "");
        }
      }
      children.push(...roots.map((key) => idFor(nodeKey(key))));
    }
    if (nodes.length >= 1024) {
      this.sample = undefined;
      this.availability =
        "Application exceeds the mobile view limit. No controls are active.";
      return this.rebuild();
    }
    add("root", { role: "root", label: "Desktop", children });
    this.keys = keys;
    this.versions = versions;
    this.targets = targets;
    this.publication = { nodes, status: this.availability };
  }
  async capture() {
    return this.publication;
  }
  async dispatch(request) {
    const expires = Date.now() + Math.min(2000, request.deadlineMs || 0);
    if (this.busy) await this.busy;
    const target = this.targets.get(request.target);
    if (
      !target ||
      target.node.version !== request.version ||
      target.node.disabled ||
      !target.node.actions.includes(request.action) ||
      Date.now() >= expires ||
      this.closed
    )
      return { status: "rejected" };
    if (target.select) {
      if (request.action !== "Activate" || !this.approved.has(target.select))
        return { status: "rejected" };
      this.select(target.select);
      void this.refresh();
      return { status: "dispatched" };
    }
    this.busy = this.invoke({
      op: "dispatch",
      source: target.source,
      key: target.key,
      fingerprint: target.fingerprint,
      action: request.action,
      value: request.value,
      expires,
    });
    try {
      const result = await this.busy;
      // Publish the new observation before delivering the command result, so
      // subsequent actions cannot use the pre-edit application fingerprint.
      this.sample = result.capture;
      if (!this.sample)
        this.availability = "Refreshing application after the action…";
      this.rebuild();
      return { status: result.status };
    } catch {
      this.sample = undefined;
      this.availability = "Action outcome unknown. Refreshing the application…";
      this.rebuild();
      return { status: "unknown" };
    } finally {
      this.busy = undefined;
      void this.refresh();
    }
  }
  stage() {
    throw Error("Desktop password input is unavailable");
  }
}
