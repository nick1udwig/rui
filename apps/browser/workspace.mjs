import { SemanticNodes } from "../shared/semantic-nodes.mjs";
import { chromium } from "playwright";
import { randomBytes } from "node:crypto";
import { readdir, realpath, lstat, mkdir } from "node:fs/promises";
import { resolve, basename, sep } from "node:path";

const MAX_NODES = 900,
  MAX_TEXT = 16_384;
const clean = (value) => String(value ?? "").slice(0, MAX_TEXT / 4);
export function inside(root, path) {
  return path === root || path.startsWith(root + sep);
}
export async function safeFile(root, path) {
  const actual = await realpath(path);
  if (!inside(root, actual) || (await lstat(path)).isSymbolicLink())
    throw Error("File is outside the shared directory");
  return actual;
}
export class Workspace {
  constructor(context, files, downloads) {
    this.context = context;
    this.files = files;
    this.downloadDir = downloads;
    this.semantic = new SemanticNodes();
    this.pages = new Map();
    this.active = undefined;
    this.offset = 0;
    this.dialog = undefined;
    this.filePicker = undefined;
    this.downloads = [];
    this.secrets = new Map();
    this.closed = false;
    context.on("page", (p) => this.register(p));
    context.on("close", () => {
      this.closed = true;
      this.secrets.clear();
    });
    for (const p of context.pages()) this.register(p);
  }
  static async launch(directory) {
    const files = await realpath(process.env.RUI_FILES_DIR);
    const downloads = resolve(directory, "downloads");
    await mkdir(downloads, { recursive: true });
    const context = await chromium.launchPersistentContext(
      resolve(directory, "profile"),
      {
        executablePath: process.env.RUI_CHROMIUM || "/usr/bin/chromium",
        headless: process.env.RUI_HEADLESS === "1",
        chromiumSandbox: true,
        // The worker closes the context before exiting so profile data flushes.
        handleSIGINT: false,
        handleSIGTERM: false,
        handleSIGHUP: false,
        viewport: null,
        acceptDownloads: true,
        downloadsPath: downloads,
        args: ["--force-renderer-accessibility", "--no-first-run"],
      },
    );
    return new Workspace(context, files, downloads);
  }
  register(page) {
    if (this.pages.has(page)) return;
    if (this.pages.size >= 16) {
      void page.close().catch(() => {});
      return;
    }
    const entry = {
      id: randomBytes(12).toString("hex"),
      generation: 1,
      sessions: new Map(),
    };
    this.pages.set(page, entry);
    this.active = page;
    page.on("framenavigated", () => {
      entry.generation++;
      this.secrets.clear();
      this.filePicker = undefined;
      this.offset = 0;
    });
    page.on("framedetached", (frame) => {
      const session = entry.sessions.get(frame);
      entry.sessions.delete(frame);
      if (session) void session.detach().catch(() => {});
    });
    page.on("close", () => {
      this.pages.delete(page);
      if (this.active === page) this.active = this.pages.keys().next().value;
      this.secrets.clear();
    });
    page.on("crash", () => {
      entry.generation++;
      this.secrets.clear();
    });
    page.on("dialog", (dialog) => {
      this.dialog = { dialog, page, id: randomBytes(8).toString("hex") };
    });
    page.on("download", (download) => {
      if (this.downloads.length >= 64) {
        void download.cancel();
        return;
      }
      const item = {
        id: randomBytes(8).toString("hex"),
        name:
          basename(download.suggestedFilename())
            .replace(/[^\p{L}\p{N}._ -]/gu, "_")
            .slice(0, 128) || "download",
        status: "Downloading",
        download,
      };
      this.downloads.push(item);
      const path = resolve(this.downloadDir, `${item.id}-${item.name}`);
      void download.saveAs(path).then(
        () => {
          item.status = `Saved on host: ${path}`;
        },
        () => {
          item.status = "Download failed or cancelled";
        },
      );
    });
  }
  async session(page, frame) {
    const entry = this.pages.get(page);
    if (!entry.sessions.has(frame))
      entry.sessions.set(frame, await this.context.newCDPSession(frame));
    return entry.sessions.get(frame);
  }
  async frames(page) {
    const result = new Map();
    for (const frame of page.frames().slice(0, 16)) {
      try {
        const session = await this.session(page, frame);
        const tree = (await session.send("Page.getFrameTree")).frameTree;
        const visit = (item) => {
          result.set(item.frame.id, { frame, session, frameId: item.frame.id });
          for (const child of item.childFrames || []) visit(child);
        };
        visit(tree);
      } catch {
        /* Local frames share the parent's CDP session. */
      }
    }
    return [...result.values()].slice(0, 16);
  }
  async object(session, backendNodeId) {
    return (await session.send("DOM.resolveNode", { backendNodeId })).object
      .objectId;
  }
  async inspect(session, backendNodeId) {
    const { node } = await session.send("DOM.describeNode", {
      backendNodeId,
      depth: 0,
    });
    const attributes = Object.fromEntries(
      Array.from({ length: (node.attributes?.length || 0) / 2 }, (_, i) => [
        node.attributes[i * 2],
        node.attributes[i * 2 + 1],
      ]),
    );
    return { tag: node.localName, attributes };
  }
  async call(session, backendNodeId, functionDeclaration, args = [], page) {
    const objectId = await this.object(session, backendNodeId);
    // A synchronous alert/prompt blocks the evaluated function until answered.
    // Receipt of the browser dialog proves dispatch; capture can now publish it.
    let onDialog;
    const opened = new Promise((resolve) => {
      if (page) {
        onDialog = () => resolve({ result: { value: true } });
        page.once("dialog", onDialog);
      }
    });
    try {
      const invoked = session.send("Runtime.callFunctionOn", {
        objectId,
        functionDeclaration,
        arguments: args.map((value) => ({ value })),
        returnByValue: true,
        userGesture: true,
      });
      const result = await (page ? Promise.race([invoked, opened]) : invoked);
      if (result.exceptionDetails) throw Error("Browser operation failed");
      return result.result.value;
    } finally {
      if (onDialog) page.off("dialog", onDialog);
      void session.send("Runtime.releaseObject", { objectId }).catch(() => {});
    }
  }
  async capture() {
    this.semantic.targets.clear();
    for (const [token, s] of this.secrets)
      if (s.expires < Date.now()) this.secrets.delete(token);
    const nodes = [],
      rootChildren = [];
    const add = (n) => {
      nodes.push(n);
      return n.id;
    };
    const group = (key, label, children, kind = null) =>
      add(this.semantic.node(key, { role: "group", label, children, kind }));
    const button = (key, label, handler, disabled = false) =>
      add(
        this.semantic.node(
          key,
          { role: "button", label, actions: ["Activate"], disabled },
          handler,
        ),
      );
    let status =
      "Observed page semantics. Images, canvas, video and unsupported widgets are unavailable.";
    if (this.closed) {
      return {
        nodes: [
          this.semantic.node("root", {
            role: "root",
            label: "Browser closed. Restart the host to continue.",
          }),
        ],
        status: "Unavailable",
      };
    }
    for (const page of this.pages.keys()) {
      if (
        page !== this.active &&
        !this.dialog &&
        (await page
          .evaluate(
            () => document.visibilityState === "visible" && document.hasFocus(),
          )
          .catch(() => false))
      ) {
        this.active = page;
        this.offset = 0;
      }
    }
    const page = this.active;
    const toolbar = [],
      tools = [];
    if (page && !page.isClosed()) {
      const entry = this.pages.get(page),
        prefix = `${entry.id}:${entry.generation}`;
      toolbar.push(
        add(
          this.semantic.node(
            `${prefix}:url`,
            {
              role: "text",
              label: "Address",
              kind: "url",
              value: page.url(),
              actions: ["SetValue"],
            },
            { op: "navigate", page },
          ),
        ),
      );
      for (const [op, label] of [
        ["back", "Back"],
        ["forward", "Forward"],
        ["reload", "Reload"],
      ])
        toolbar.push(button(`${prefix}:${op}`, label, { op, page }));
      tools.push(
        button(
          `${prefix}:previous`,
          "Previous page section",
          { op: "section", delta: -400 },
          this.offset === 0,
        ),
      );
      tools.push(
        button(`${prefix}:next`, "Next page section", {
          op: "section",
          delta: 400,
        }),
      );
      tools.push(
        button(`${prefix}:scroll`, "Scroll host page down", {
          op: "scroll",
          page,
        }),
      );
    }
    tools.push(button("new-tab", "New tab", { op: "new" }));
    rootChildren.push(group("toolbar", "Browser", toolbar, "view:toolbar"));
    const tabs = [];
    for (const [p, entry] of this.pages) {
      if (!this.dialog) entry.title = await p.title().catch(() => "Tab");
      tabs.push(
        button(
          `${entry.id}:switch`,
          `${p === page ? "Current: " : ""}${clean(entry.title || "Tab") || p.url()}`,
          { op: "switch", page: p },
        ),
      );
      tabs.push(
        button(`${entry.id}:close`, "Close tab", { op: "close", page: p }),
      );
    }
    rootChildren.push(group("tabs", "Tabs", tabs, "view:tabs"));
    rootChildren.push(
      group("browser-tools", "More browser controls", tools, "view:disclosure"),
    );
    if (this.dialog) {
      const d = this.dialog,
        children = [];
      children.push(
        add(
          this.semantic.node(`${d.id}:message`, {
            label: clean(d.dialog.message()),
          }),
        ),
      );
      if (d.dialog.type() === "prompt")
        children.push(
          add(
            this.semantic.node(
              `${d.id}:prompt`,
              {
                role: "text",
                label: "Prompt response",
                kind: "text",
                value: d.dialog.defaultValue(),
                actions: ["SetValue"],
              },
              { op: "prompt", dialog: d },
            ),
          ),
        );
      else
        children.push(
          button(`${d.id}:accept`, "Accept dialog", {
            op: "dialog",
            accept: true,
            dialog: d,
          }),
        );
      children.push(
        button(`${d.id}:dismiss`, "Dismiss dialog", {
          op: "dialog",
          accept: false,
          dialog: d,
        }),
      );
      rootChildren.push(
        group(`${d.id}:dialog`, "Page dialog", children, "view:dialog"),
      );
    } else if (page && !page.isClosed()) {
      const content = [];
      let count = 0;
      let included = 0;
      for (const { frame, session, frameId } of await this.frames(page)) {
        try {
          const tree = await session.send("Accessibility.getFullAXTree", {
            frameId,
            depth: 32,
          });
          const map = new Map(tree.nodes.map((n) => [n.nodeId, n]));
          const visit = async (ax) => {
            if (!ax || included >= 600 || nodes.length >= MAX_NODES - 40)
              return;
            const role = ax.role?.value,
              label = clean(ax.name?.value),
              backend = ax.backendDOMNodeId;
            const props = Object.fromEntries(
              (ax.properties || []).map((p) => [p.name, p.value.value]),
            );
            const interactive =
              !!props.editable ||
              [
                "button",
                "link",
                "checkbox",
                "radio",
                "textbox",
                "searchbox",
                "combobox",
                "spinbutton",
                "slider",
                "menuitem",
                "tab",
                "DisclosureTriangle",
                "Date",
                "DateTime",
                "InputTime",
              ].includes(role);
            const structure = {
              main: "document",
              article: "document",
              navigation: "navigation",
              list: "list",
              listitem: "item",
              table: "table",
              row: "row",
              cell: "cell",
              columnheader: "cell",
              rowheader: "cell",
              paragraph: "paragraph",
              blockquote: "quote",
            }[role];
            const start = content.length;
            const informational = [
              "StaticText",
              "heading",
              "alert",
              "status",
              "img",
            ].includes(role);
            if (
              !ax.ignored &&
              (interactive || informational) &&
              (label || interactive)
            ) {
              const index = count++;
              if (index >= this.offset && included < 400) {
                const prefix = `${this.pages.get(page).id}:${this.pages.get(page).generation}:${frameId}:${backend || ax.nodeId}`;
                let node = {
                    label,
                    role: "label",
                    kind:
                      role === "heading"
                        ? `view:heading${Math.max(1, Math.min(6, Number(props.level) || 2))}`
                        : ["status", "alert"].includes(role)
                          ? "view:status"
                          : null,
                  },
                  handler;
                if (interactive && backend) {
                  const { tag, attributes } = await this.inspect(
                    session,
                    backend,
                  );
                  const type = (attributes.type || "").toLowerCase();
                  const base = {
                    page,
                    frame,
                    session,
                    backend,
                    expected: String(ax.value?.value ?? ""),
                    tag,
                    type,
                    name: label,
                  };
                  node.disabled =
                    props.disabled === true ||
                    attributes.disabled !== undefined ||
                    props.readonly === true;
                  if (type === "file") {
                    node = {
                      ...node,
                      role: "button",
                      kind: "file",
                      actions: ["Activate"],
                    };
                    handler = { ...base, op: "file" };
                  } else if (tag === "select") {
                    const options = await this.call(
                      session,
                      backend,
                      "function(){return Array.from(this.options).slice(0,128).map((o,i)=>({label:o.label,selected:o.selected,disabled:o.disabled,index:i,value:o.value}));}",
                    );
                    const children = [];
                    for (const option of options)
                      children.push(
                        button(
                          `${prefix}:option:${option.index}:${option.value}`,
                          `${option.selected ? "Selected: " : ""}${clean(option.label)}`,
                          {
                            ...base,
                            op: "option",
                            option: option.index,
                            optionValue: option.value,
                          },
                          node.disabled || option.disabled,
                        ),
                      );
                    content.push(
                      group(prefix, label || "Choose an option", children),
                    );
                    included++;
                    return;
                  } else if (role === "checkbox") {
                    node = {
                      ...node,
                      role: "checkbox",
                      value: props.checked === "true" || props.checked === true,
                      actions: ["SetValue"],
                    };
                    handler = { ...base, op: "checked", checked: node.value };
                  } else if (
                    props.editable ||
                    ["textbox", "searchbox", "spinbutton", "slider"].includes(
                      role,
                    ) ||
                    tag === "textarea" ||
                    (tag === "input" &&
                      ![
                        "button",
                        "submit",
                        "reset",
                        "radio",
                        "checkbox",
                      ].includes(type))
                  ) {
                    const allowedTypes = [
                      "text",
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
                      "range",
                    ];
                    const kind =
                      tag === "textarea" || tag !== "input"
                        ? "textarea"
                        : allowedTypes.includes(type)
                          ? type
                          : "text";
                    const source = await this.call(
                      session,
                      backend,
                      'function(){return {value:this.type==="password"?"":String(this.value??this.innerText??""),plain:!this.isContentEditable||this.contentEditable==="plaintext-only"||this.children.length===0};}',
                    );
                    const tooLarge = Buffer.byteLength(source.value) > MAX_TEXT;
                    node = {
                      ...node,
                      role: "text",
                      kind,
                      value: tooLarge ? "" : source.value,
                      actions: ["SetValue"],
                      disabled: node.disabled || tooLarge || !source.plain,
                      description: tooLarge
                        ? "Field exceeds the 16 KiB editing limit"
                        : !source.plain
                          ? "Rich text formatting is unsupported"
                          : "",
                    };
                    handler = {
                      ...base,
                      expected: source.value,
                      op: "value",
                      secret: kind === "password",
                    };
                  } else {
                    node = {
                      ...node,
                      role: "button",
                      kind:
                        role === "link"
                          ? "link"
                          : role === "radio"
                            ? "radio"
                            : null,
                      actions: ["Activate"],
                      description:
                        role === "radio" &&
                        (props.checked === "true" || props.checked === true)
                          ? "Selected"
                          : "",
                    };
                    handler = { ...base, op: "click" };
                  }
                } else if (role === "img")
                  node.description = "Image description; image unavailable";
                // AX exposes label text separately from its labelled control.
                const previous = nodes.find((n) => n.id === content.at(-1));
                if (
                  previous?.role === "label" &&
                  previous.label === node.label &&
                  node.role !== "label"
                ) {
                  content.pop();
                  nodes.splice(nodes.indexOf(previous), 1);
                }
                if (
                  !(
                    node.role === "label" &&
                    previous?.role === "label" &&
                    previous.label === node.label
                  )
                )
                  content.push(add(this.semantic.node(prefix, node, handler)));
                included++;
              }
              if (interactive) return;
            }
            for (const id of ax.childIds || []) await visit(map.get(id));
            if (
              structure &&
              !ax.ignored &&
              content.length > start &&
              nodes.length < MAX_NODES - 40
            ) {
              const children = content.splice(start);
              const key = `${this.pages.get(page).id}:${this.pages.get(page).generation}:${frameId}:structure:${ax.nodeId}`;
              content.push(group(key, label, children, `view:${structure}`));
            }
          };
          await visit(tree.nodes[0]);
        } catch {
          status =
            "Partial page observation: a frame is unavailable or navigating.";
        }
      }
      if (count > this.offset + 400)
        status += " More content is available in the next page section.";
      if (!content.length)
        content.push(
          add(
            this.semantic.node("empty-content", {
              label:
                "No supported page content in this section. Try the previous section or another address.",
            }),
          ),
        );
      rootChildren.push(
        group("page-content", "Page", content, "view:document"),
      );
    }
    if (this.filePicker) {
      const picker = this.filePicker,
        children = [];
      const directory = await safeFile(this.files, picker.directory);
      if (directory !== this.files)
        children.push(
          button(`files:${directory}:up`, "Parent folder", {
            op: "folder",
            path: resolve(directory, ".."),
          }),
        );
      for (const item of (
        await readdir(directory, { withFileTypes: true })
      ).slice(0, 128)) {
        if (item.isSymbolicLink() || (!item.isDirectory() && !item.isFile()))
          continue;
        children.push(
          button(
            `files:${picker.backend}:${directory}:${item.name}`,
            `${item.isDirectory() ? "Folder: " : ""}${item.name}`,
            {
              op: item.isDirectory() ? "folder" : "choose-file",
              path: resolve(directory, item.name),
              picker,
            },
          ),
        );
      }
      children.push(
        button("file-cancel", "Cancel file selection", { op: "cancel-file" }),
      );
      rootChildren.push(group("file-picker", "Choose a host file", children));
    }
    if (this.downloads.length) {
      const children = [];
      for (const d of this.downloads) {
        children.push(
          add(
            this.semantic.node(`${d.id}:download`, {
              label: d.name,
              description: d.status,
            }),
          ),
        );
        if (d.status === "Downloading")
          children.push(
            button(`${d.id}:cancel`, "Cancel download", {
              op: "cancel-download",
              download: d.download,
            }),
          );
      }
      rootChildren.push(group("downloads", "Host downloads", children));
    }
    nodes.unshift(
      this.semantic.node("root", {
        role: "root",
        label: "Remote browser",
        children: rootChildren,
      }),
    );
    this.semantic.retain(nodes);
    const capture = { nodes, status };
    if (Buffer.byteLength(JSON.stringify(capture)) > 900_000)
      throw Error("Page semantic limit");
    return capture;
  }
  stage(scope, secret) {
    if (
      typeof secret !== "string" ||
      Buffer.byteLength(secret) > 16384 ||
      this.secrets.size >= 16
    )
      throw Error("Secret limit");
    const token = randomBytes(32).toString("hex");
    this.secrets.set(token, { ...scope, secret, expires: Date.now() + 60_000 });
    return token;
  }
  async dispatch(request) {
    const deadline = Date.now() + Math.min(2000, request.deadlineMs || 0);
    try {
      // Refresh before resolving a public identity; a changed document never inherits it.
      await this.capture();
      const target = this.semantic.targets.get(request.target);
      if (
        !target ||
        target.node.version !== request.version ||
        target.node.disabled ||
        !target.node.actions.includes(request.action) ||
        Date.now() >= deadline
      )
        return { status: "rejected" };
      let value = request.value;
      if (target.secret) {
        const s = this.secrets.get(value);
        this.secrets.delete(value);
        if (
          !s ||
          s.expires < Date.now() ||
          ["client", "target", "version", "lease", "generation", "epoch"].some(
            (k) => s[k] !== request[k],
          )
        )
          return { status: "rejected" };
        value = s.secret;
        s.secret = "";
      }
      const page = target.page;
      switch (target.op) {
        case "navigate": {
          const url = new URL(value);
          if (
            !["http:", "https:", "about:"].includes(url.protocol) ||
            (url.protocol === "about:" && value !== "about:blank")
          )
            return { status: "rejected" };
          await page.goto(url.href, { waitUntil: "commit", timeout: 1500 });
          break;
        }
        case "back":
          await page.goBack({ waitUntil: "commit", timeout: 1500 });
          break;
        case "forward":
          await page.goForward({ waitUntil: "commit", timeout: 1500 });
          break;
        case "reload":
          await page.reload({ waitUntil: "commit", timeout: 1500 });
          break;
        case "new":
          if (this.pages.size >= 16) return { status: "rejected" };
          await this.context.newPage();
          break;
        case "switch":
          this.active = page;
          this.offset = 0;
          this.filePicker = undefined;
          await page.bringToFront();
          break;
        case "close":
          await page.close();
          break;
        case "section":
          this.offset = Math.max(0, this.offset + target.delta);
          break;
        case "scroll":
          await page.evaluate(() => window.scrollBy(0, innerHeight * 0.8));
          break;
        case "prompt":
          await target.dialog.dialog.accept(String(value));
          this.dialog = undefined;
          break;
        case "dialog":
          await (target.accept
            ? target.dialog.dialog.accept()
            : target.dialog.dialog.dismiss());
          this.dialog = undefined;
          break;
        case "file":
          this.filePicker = { ...target, directory: this.files };
          break;
        case "folder":
          this.filePicker.directory = await safeFile(this.files, target.path);
          break;
        case "cancel-file":
          this.filePicker = undefined;
          break;
        case "choose-file": {
          const file = await safeFile(this.files, target.path);
          if (!(await lstat(file)).isFile()) return { status: "rejected" };
          await target.picker.session.send("DOM.setFileInputFiles", {
            backendNodeId: target.picker.backend,
            files: [file],
          });
          this.filePicker = undefined;
          break;
        }
        case "cancel-download":
          await target.download.cancel();
          break;
        default: {
          const result = await this.call(
            target.session,
            target.backend,
            `function(op, value, expected, option, optionValue, deadline, secret) {
            if (Date.now()>=deadline || !this.isConnected || this.disabled || this.readOnly || this.closest('[inert]')) return false;
            if (op==='value') {
              if (!secret && String(this.value ?? this.innerText ?? '')!==expected) return false;
              this.focus();
              if (this instanceof HTMLInputElement || this instanceof HTMLTextAreaElement) {
                const proto=this instanceof HTMLInputElement?HTMLInputElement.prototype:HTMLTextAreaElement.prototype;
                const probe=this.cloneNode();Object.getOwnPropertyDescriptor(proto,'value').set.call(probe,String(value));
                if(probe.value!==String(value)||probe.validity?.rangeOverflow||probe.validity?.rangeUnderflow||probe.validity?.stepMismatch||probe.validity?.tooLong)return false;
                Object.getOwnPropertyDescriptor(proto,'value').set.call(this,String(value));
              } else if(this.isContentEditable) this.textContent=String(value); else return false;
              this.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertReplacementText'}));this.dispatchEvent(new Event('change',{bubbles:true}));
            } else if(op==='checked') {if(this.checked!==expected)return false;if(this.checked!==value)this.click();}
            else if(op==='option') {if(!this.options[option]||this.options[option].value!==optionValue||this.options[option].disabled)return false;this.options[option].selected=!this.multiple||!this.options[option].selected;this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));}
            else this.click(); return true;
          }`,
            [
              target.op,
              value,
              target.op === "checked" ? target.checked : target.expected,
              target.option ?? null,
              target.optionValue ?? null,
              deadline,
              !!target.secret,
            ],
            page,
          );
          if (!result) return { status: "rejected" };
        }
      }
      return { status: "dispatched" };
    } catch {
      return { status: "unknown" };
    }
  }
}
