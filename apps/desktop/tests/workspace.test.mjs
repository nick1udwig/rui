import { test } from "node:test";
import assert from "node:assert/strict";
import { DesktopWorkspace } from "../workspace.mjs";

function fixture() {
  const source = {
    root: [":1.2", "/app"],
    pid: 100,
    start: "200",
    name: "Notes",
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
