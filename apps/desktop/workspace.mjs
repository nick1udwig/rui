import { NativeWorker } from "./native-worker.mjs";
import { indexTree } from "./tree.mjs";
import { SemanticNodes } from "../shared/semantic-nodes.mjs";
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
          if (result.error)
            throw Error(
              result.detail ? `${result.error} ${result.detail}` : result.error,
            );
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
    const browser =
      process.env.RUI_DESKTOP_BROWSER === "1"
        ? await (
            await import("./browser.mjs")
          ).DesktopBrowser.launch(process.env.RUI_STATE)
        : undefined;
    const worker = new NativeWorker(() => {
      workspace.nativeDirty = true;
      void workspace.refresh();
    });
    const workspace = new DesktopWorkspace(
      (request) => worker.request(request),
      browser,
    );
    workspace.worker = worker;
    // Fail startup with a useful error when not launched in a desktop session.
    try {
      await workspace.local({ op: "apps" });
    } catch (error) {
      worker.close();
      await browser?.close();
      throw error;
    }
    workspace.timer = setInterval(() => void workspace.refresh(), 500);
    return workspace;
  }
  constructor(invoke = native, browser) {
    this.browser = browser;
    this.invoke = (request) =>
      request.source?.adapter === "browser"
        ? browser.request(request)
        : invoke(request);
    this.catalog = new Map();
    this.privatePids = new Set();
    this.approved = new Map();
    this.policyRevision = 0;
    this.sessionSharing = false;
    this.excluded = new Set();
    this.catalogRead = 0;
    this.selected = undefined;
    this.generation = 0;
    this.windowGeneration = 0;
    this.preferredWindow = undefined;
    this.activeWindow = undefined;
    this.semantic = new SemanticNodes();
    this.sample = undefined;
    this.busy = undefined;
    this.closed = false;
    this.availability =
      "No applications shared. Use apps, then share ID in the host terminal.";
    this.context = { close: () => this.close() };
    this.rebuild();
  }
  close() {
    if (this.closing) return this.closing;
    this.closed = true;
    this.policyRevision++;
    clearInterval(this.timer);
    this.worker?.close();
    this.approved.clear();
    this.select(undefined);
    return (this.closing = (async () => {
      try {
        await this.busy;
      } finally {
        await this.browser?.close();
      }
    })());
  }
  async local(request) {
    if (this.closed) throw Error("Desktop workspace is closed");
    if (request.op === "apps") {
      const revision = this.policyRevision;
      const result = await this.invoke({ op: "list" });
      if (this.closed || revision !== this.policyRevision)
        throw Error("Sharing policy changed during discovery");
      if (!Array.isArray(result.apps) || result.apps.length > 64)
        throw Error("Application limit");
      if (this.browser && !this.browser.workspace.closed)
        result.apps = [...result.apps, this.browser.source];
      if (result.apps.length > 65) throw Error("Application limit");
      this.catalog = new Map(
        result.apps
          .filter(
            (source) => !this.privatePids.has(source.pid) && !source.local_only,
          )
          .map((source) => {
            const id =
              source.adapter === "browser"
                ? "browser"
                : createHash("sha256")
                    .update(
                      JSON.stringify([source.root, source.pid, source.start]),
                    )
                    .digest("hex")
                    .slice(0, 16);
            return [id, source];
          }),
      );
      this.catalogRead = Date.now();
      if (this.sessionSharing) {
        const next = new Map(
          [...this.catalog].filter(([id]) => !this.excluded.has(id)),
        );
        this.approved = next;
        if (!next.has(this.selected)) this.select(next.keys().next().value);
        this.rebuild();
      }
      return {
        sessionSharing: this.sessionSharing,
        apps: [...new Map([...this.approved, ...this.catalog])].map(
          ([id, source]) => ({
            id,
            name: source.name,
            pid: source.pid,
            shared: this.approved.has(id),
            available: this.catalog.has(id),
          }),
        ),
      };
    }
    if (request.op === "session") {
      if (!["on", "off"].includes(request.id))
        throw Error("Use session on or session off");
      this.policyRevision++;
      this.sessionSharing = request.id === "on";
      this.excluded.clear();
      if (this.sessionSharing) {
        const revision = this.policyRevision;
        try {
          return await this.local({ op: "apps" });
        } catch (error) {
          if (revision === this.policyRevision) this.sessionSharing = false;
          throw error;
        }
      }
      this.approved.clear();
      this.select(undefined);
      return { sessionSharing: false, apps: [] };
    }
    if (request.op === "protect") {
      if (!/^[1-9][0-9]*$/.test(request.id || ""))
        throw Error("Invalid owner PID");
      this.policyRevision++;
      const pid = Number(request.id);
      this.privatePids.add(pid);
      for (const [id, source] of this.catalog)
        if (source.pid === pid) this.catalog.delete(id);
      for (const [id, source] of this.approved)
        if (source.pid === pid) await this.local({ op: "unshare", id });
      return { protected: true };
    }
    if (request.op === "diagnose") {
      const source = this.catalog.get(request.id);
      if (!source || this.privatePids.has(source.pid))
        throw Error("Application is no longer discoverable; refresh the list");
      return this.invoke({ op: "diagnose", source });
    }
    if (request.op === "share") {
      const source = this.catalog.get(request.id);
      if (!source || this.privatePids.has(source.pid))
        throw Error("Run apps and use an application ID from that list");
      if (this.approved.size >= 65 && !this.approved.has(request.id))
        throw Error("64 native applications plus the managed browser maximum");
      const revision = ++this.policyRevision;
      const sample = await this.invoke({ op: "capture", source });
      if (
        this.closed ||
        revision !== this.policyRevision ||
        this.privatePids.has(source.pid)
      )
        throw Error("Sharing policy changed while reading application");
      this.excluded.delete(request.id);
      this.approved.set(request.id, source);
      this.select(request.id);
      this.sample = sample;
      this.availability =
        sample.status ||
        "Observed accessibility content. Graphics and password fields are unavailable.";
      this.rebuild();
      return { shared: source.name, id: request.id };
    }
    if (request.op === "unshare") {
      this.policyRevision++;
      if (this.sessionSharing) this.excluded.add(request.id);
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
    this.nativeDirty = true;
    this.browser?.clearSecrets();
    this.selected = id;
    this.preferredWindow = undefined;
    this.activeWindow = undefined;
    this.generation++;
    this.sample = undefined;
    this.availability = id
      ? "Reading application…"
      : "No applications shared. Use apps, then share ID in the host terminal.";
    this.rebuild();
  }
  async refresh() {
    if (this.closed || this.busy) return;
    if (this.sessionSharing && Date.now() - this.catalogRead > 3000) {
      this.busy = this.local({ op: "apps" });
      try {
        await this.busy;
      } catch {
        this.catalogRead = Date.now();
      } finally {
        this.busy = undefined;
      }
    }
    if (!this.selected || this.closed) return;
    const id = this.selected,
      generation = this.generation,
      source = this.approved.get(id);
    if (this.worker && source?.adapter !== "browser") {
      if (!this.nativeDirty && Date.now() - (this.nativeRead ?? 0) < 2000)
        return;
      this.nativeDirty = false;
      this.nativeRead = Date.now();
    }
    this.busy = (async () => {
      try {
        const sample = await this.invoke({ op: "capture", source });
        if (this.generation !== generation || this.closed) return;
        this.sample = sample;
        this.availability =
          sample.status ||
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
    let tree;
    if (this.sample) {
      try {
        tree = indexTree(this.sample.nodes);
      } catch {
        this.sample = undefined;
        this.availability =
          "Application returned an invalid or oversized tree. Retrying; no controls are active.";
      }
    }
    const semantic = new SemanticNodes(this.semantic),
      nodes = [];
    const idFor = (key) => semantic.id(key);
    const add = (key, data, handler, guard) => {
      const node = semantic.node(key, data, handler, guard);
      nodes.push(node);
      return node.id;
    };
    const apps = [];
    for (const [id, source] of this.approved) {
      apps.push(
        add(
          `app:${id}`,
          {
            role: "button",
            kind: id === this.selected ? "view:app-active" : "view:app",
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
        kind: "view:apps",
        children: apps,
      }),
    ];
    if (this.sample && this.selected) {
      const source = this.approved.get(this.selected);
      // Drop layout-only containers and duplicated button labels from the
      // mobile presentation. Dispatch still validates the full native tree.
      const { byKey, keyOf, encoded, parentOf } = tree;
      const windows = this.sample.nodes.filter((node) => {
        if (!node.window) return false;
        let parent = parentOf(node);
        while (parent) {
          if (parent.window) return false;
          parent = parentOf(parent);
        }
        return true;
      });
      const modals = this.sample.nodes.filter((node) => node.modal);
      const chosen =
        modals.length === 1
          ? modals[0]
          : (windows.find((node) => encoded(node) === this.preferredWindow) ??
            windows[0] ??
            this.sample.nodes[0]);
      const currentWindow = encoded(chosen);
      if (this.activeWindow !== currentWindow) {
        this.activeWindow = currentWindow;
        this.windowGeneration++;
      }
      if (!modals.length) this.preferredWindow = currentWindow;
      const prefix = `${this.generation}:${this.windowGeneration}:${this.selected}:`;
      const nodeKey = (key) => prefix + keyOf(key);
      if (windows.length || modals.length) {
        const navigation = [...windows];
        if (!navigation.includes(chosen)) navigation.push(chosen);
        children.push(
          add("windows", {
            role: "group",
            label: modals.length ? "Dialog requires attention" : "Windows",
            kind: "view:windows",
            children: navigation.map((node, index) =>
              add(
                `window:${this.generation}:${encoded(node)}`,
                {
                  role: "button",
                  kind: node === chosen ? "view:window-active" : "view:window",
                  label: `${node === chosen ? "Window: " : "Show window: "}${node.label || `Untitled ${index + 1}`}`,
                  actions: modals.length ? [] : ["Activate"],
                  disabled: !!modals.length,
                },
                { window: encoded(node) },
              ),
            ),
          }),
        );
      }
      const visible = [];
      const present = (raw, appRoot = false) => {
        const childKeys = raw.children.flatMap((key) => {
          const child = byKey.get(keyOf(key));
          if (
            (raw.role === "button" || raw.role === "group") &&
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
          (raw.role === "group" &&
            !raw.label &&
            !raw.actions.length &&
            !raw.presentation &&
            !raw.kind)
        )
          return childKeys;
        visible.push({ ...raw, children: childKeys });
        return [raw.key];
      };
      const roots = present(chosen, true);
      for (const raw of visible) {
        const key = nodeKey(raw.key),
          structural = raw.children.length > 0;
        const node = {
          role: raw.role,
          label:
            raw.native_action === "listitem.scroll-to"
              ? `Scroll to: ${raw.label}`
              : raw.label ||
                (raw.role === "text"
                  ? "Text"
                  : raw.role === "button"
                    ? "Activate"
                    : ""),
          description: raw.description || "",
          actions: raw.actions,
          disabled: raw.disabled,
          kind: raw.kind || raw.presentation || null,
          ...(raw.value !== undefined ? { value: raw.value } : {}),
          children: raw.children.map((k) => idFor(nodeKey(k))),
        };
        const handler = raw.actions.length
          ? { source, key: raw.key, fingerprint: raw.fingerprint }
          : undefined;
        if (structural && (raw.role === "text" || raw.actions.length)) {
          const content = add(
            key + ":control",
            { ...node, children: [] },
            handler,
            raw.fingerprint,
          );
          add(key, {
            role: "group",
            label: raw.label,
            kind: raw.presentation || null,
            children: [content, ...node.children],
          });
        } else {
          add(key, node, handler, handler ? raw.fingerprint : "");
        }
      }
      children.push(
        add("content", {
          role: "group",
          label: chosen.label || source.name,
          kind: modals.length ? "view:dialog" : "view:content",
          children: roots.map((key) => idFor(nodeKey(key))),
        }),
      );
    }
    if (nodes.length >= 1024) {
      this.sample = undefined;
      this.availability =
        "Application exceeds the mobile view limit. No controls are active.";
      return this.rebuild();
    }
    add("root", { role: "root", label: "Desktop", children });
    semantic.retain(nodes);
    this.semantic = semantic;
    this.publication = { nodes, status: this.availability };
  }
  async capture() {
    return this.publication;
  }
  async dispatch(request) {
    const expires = Date.now() + Math.min(2000, request.deadlineMs || 0);
    if (this.busy) await this.busy;
    const target = this.semantic.targets.get(request.target);
    if (
      !target ||
      target.node.version !== request.version ||
      target.node.disabled ||
      !target.node.actions.includes(request.action) ||
      Date.now() >= expires ||
      this.closed
    ) {
      if (process.env.RUI_DESKTOP_TRACE)
        console.error(
          "Desktop dispatch:",
          "rejected",
          !target
            ? "missing public target"
            : target.node.version !== request.version
              ? "changed public target"
              : Date.now() >= expires
                ? "public deadline"
                : "disabled public action",
        );
      return { status: "rejected" };
    }
    if (target.select) {
      if (request.action !== "Activate" || !this.approved.has(target.select))
        return { status: "rejected" };
      this.select(target.select);
      void this.refresh();
      return { status: "dispatched" };
    }
    if (target.window) {
      if (request.action !== "Activate") return { status: "rejected" };
      this.preferredWindow = target.window;
      this.rebuild();
      return { status: "dispatched" };
    }
    const generation = this.generation;
    this.busy = this.invoke({
      ...request,
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
      if (process.env.RUI_DESKTOP_TRACE)
        console.error("Desktop dispatch:", result.status, result.reason || "");
      if (this.closed || generation !== this.generation)
        return { status: result.status };
      // Publish the new observation before delivering the command result, so
      // subsequent actions cannot use the pre-edit application fingerprint.
      this.sample = result.capture;
      if (!this.sample)
        this.availability = "Refreshing application after the action…";
      this.rebuild();
      return { status: result.status };
    } catch {
      if (this.closed || generation !== this.generation)
        return { status: "unknown" };
      this.sample = undefined;
      this.availability = "Action outcome unknown. Refreshing the application…";
      this.rebuild();
      return { status: "unknown" };
    } finally {
      this.busy = undefined;
      void this.refresh();
    }
  }
  stage(scope, secret) {
    const target = this.semantic.targets.get(scope.target);
    if (
      !target ||
      target.source?.adapter !== "browser" ||
      target.node.kind !== "password" ||
      target.node.version !== scope.version ||
      !target.node.actions.includes("SetValue")
    )
      throw Error("Password input is unavailable for this target");
    return this.browser.stage(scope, secret, target);
  }
}
