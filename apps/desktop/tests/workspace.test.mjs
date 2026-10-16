import { test } from "node:test";
import assert from "node:assert/strict";
import { DesktopWorkspace } from "../workspace.mjs";

function fixture(name = "Notes", localOnly = false) {
  const source = {
    root: [":1.2", "/app"],
    pid: 100,
    start: "200",
    name,
    local_only: localOnly,
  };
  let fingerprint = "a";
  let captures;
  const calls = [];
  const invoke = async (request) => {
    calls.push(request);
    if (request.op === "list") return { apps: [source] };
    if (request.op === "capture") {
      if (captures) return captures();
      return {
        fingerprint,
        nodes: [
          {
            key: source.root,
            role: "group",
            label: "Notes",
            children: [[":1.2", "/text"]],
            actions: [],
            disabled: false,
          },
          {
            key: [":1.2", "/text"],
            role: "text",
            label: "Document",
            kind: "textarea",
            value: "Host content",
            fingerprint,
            children: [],
            actions: ["SetValue"],
            disabled: false,
          },
        ],
      };
    }
    return { status: "dispatched" };
  };
  return {
    workspace: new DesktopWorkspace(invoke),
    calls,
    change: () => (fingerprint = "b"),
    capture: (fn) => (captures = fn),
  };
}
const command = (node) => ({
  target: node.id,
  version: node.version,
  action: "SetValue",
  value: "edit",
  deadlineMs: 2000,
});
const field = async (w) =>
  (await w.capture()).nodes.find((n) => n.label === "Document");
async function share(w) {
  const { apps } = await w.local({ op: "apps" });
  await w.local({ op: "share", id: apps[0].id });
  return apps[0].id;
}

test("discovery stays local; only shared content and opaque IDs reach clients", async () => {
  const { workspace: w } = fixture();
  const listing = await w.local({ op: "apps" });
  assert.equal(
    (await w.capture()).nodes.some((n) => n.label === "Notes"),
    false,
  );
  await assert.rejects(w.local({ op: "share", id: "unknown" }));
  await w.local({ op: "share", id: listing.apps[0].id });
  const publication = await w.capture();
  assert.equal(
    publication.nodes.some((n) => n.label === "Document"),
    true,
  );
  assert.equal(JSON.stringify(publication).includes(":1.2"), false);
  assert.equal(JSON.stringify(publication).includes("/text"), false);
});

test("source changes reject old drafts; unshare retires IDs and blocks commands", async () => {
  const f = fixture(),
    w = f.workspace;
  const id = await share(w),
    old = await field(w);
  f.change();
  await w.refresh();
  assert.ok((await field(w)).version > old.version);
  assert.equal((await w.dispatch(command(old))).status, "rejected");
  const fresh = await field(w);
  await w.local({ op: "unshare", id });
  assert.equal(await field(w), undefined);
  assert.equal((await w.dispatch(command(fresh))).status, "rejected");
  await w.local({ op: "share", id });
  assert.notEqual((await field(w)).id, fresh.id);
  assert.equal(f.calls.filter((c) => c.op === "dispatch").length, 0);
});

test("an in-flight observation cannot republish an unshared application", async () => {
  const f = fixture(),
    w = f.workspace;
  const id = await share(w);
  let finish;
  f.capture(() => new Promise((resolve) => (finish = resolve)));
  const pending = w.refresh();
  await w.local({ op: "unshare", id });
  finish({ fingerprint: "old", nodes: [] });
  await pending;
  assert.equal(await field(w), undefined);
  assert.equal(w.targets.size, 0);
});

test("unavailable observations clear controls, recovery allocates fresh identities", async () => {
  const f = fixture(),
    w = f.workspace;
  await share(w);
  const old = await field(w);
  f.capture(() => {
    throw Error("stalled");
  });
  await w.refresh();
  assert.equal(await field(w), undefined);
  assert.equal((await w.dispatch(command(old))).status, "rejected");
  f.capture(undefined);
  await w.refresh();
  assert.notEqual((await field(w)).id, old.id);
});

test("ambiguous native dispatch is never reported as safely rejected", async () => {
  const f = fixture(),
    w = f.workspace;
  await share(w);
  const invoke = w.invoke;
  w.invoke = async (request) => {
    if (request.op === "dispatch")
      throw Error("worker timed out after sending");
    return invoke(request);
  };
  assert.equal((await w.dispatch(command(await field(w)))).status, "unknown");
  await w.context.close();
});

test("window navigation isolates content, retires controls and follows modals", async () => {
  const f = fixture(),
    w = f.workspace;
  const key = (name) => [":1.2", "/" + name];
  let names = ["first", "second"],
    modal = false;
  f.capture(() => ({
    nodes: [
      {
        key: key("app"),
        parent: null,
        role: "group",
        label: "Notes",
        children: names.map(key),
        actions: [],
      },
      ...names.flatMap((name) => [
        {
          key: key(name),
          parent: key("app"),
          role: "group",
          label: name,
          window: true,
          modal: modal && name === "second",
          children: [key(name + "-text")],
          actions: [],
        },
        {
          key: key(name + "-text"),
          parent: key(name),
          role: "text",
          label: name + " document",
          kind: "textarea",
          value: name,
          children: [],
          actions: ["SetValue"],
          fingerprint: name,
        },
      ]),
    ],
  }));
  await share(w);
  const find = (label) => w.publication.nodes.find((n) => n.label === label);
  const old = find("first document");
  assert.ok(old);
  assert.equal(find("second document"), undefined);
  const switcher = find("Show window: second");
  assert.equal(
    (await w.dispatch({ ...command(switcher), action: "Activate" })).status,
    "dispatched",
  );
  assert.equal(find("first document"), undefined);
  assert.ok(find("second document"));
  assert.equal((await w.dispatch(command(old))).status, "rejected");
  // Closing the selected window returns to a surviving window, with fresh IDs.
  names = ["first"];
  await w.refresh();
  assert.notEqual(find("first document").id, old.id);
  const restored = find("first document");
  names = ["first", "second"];
  modal = true;
  await w.refresh();
  assert.equal(find("first document"), undefined);
  assert.ok(find("second document"));
  assert.equal(find("Show window: first").disabled, true);
  assert.equal((await w.dispatch(command(restored))).status, "rejected");
  modal = false;
  await w.refresh();
  assert.ok(find("first document"));
});

test("local owner protection removes approvals and cannot be bypassed by stale IDs", async () => {
  const f = fixture(),
    w = f.workspace;
  const id = await share(w);
  const old = await field(w);
  await w.local({ op: "protect", id: "100" });
  assert.equal(await field(w), undefined);
  assert.equal((await w.dispatch(command(old))).status, "rejected");
  assert.deepEqual((await w.local({ op: "apps" })).apps, []);
  await assert.rejects(w.local({ op: "share", id }));
  await assert.rejects(w.local({ op: "diagnose", id }));
});

test("closed shared apps remain removable and diagnostics never approve or publish content", async () => {
  const f = fixture(),
    w = f.workspace;
  const { apps } = await w.local({ op: "apps" });
  await w.local({ op: "diagnose", id: apps[0].id });
  assert.equal(w.approved.size, 0);
  assert.equal(await field(w), undefined);
  assert.equal(f.calls.at(-1).op, "diagnose");
  const id = await share(w);
  const invoke = w.invoke;
  w.invoke = async (request) =>
    request.op === "list" ? { apps: [] } : invoke(request);
  const listing = await w.local({ op: "apps" });
  assert.equal(listing.apps[0].shared, true);
  assert.equal(listing.apps[0].available, false);
  await w.local({ op: "unshare", id });
  assert.equal(w.approved.size, 0);
});

test("the local-only sharing UI stays excluded after host restart", async () => {
  const { workspace } = fixture("Notes", true);
  assert.deepEqual((await workspace.local({ op: "apps" })).apps, []);
});
