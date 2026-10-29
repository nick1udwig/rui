import test from "node:test";
import assert from "node:assert/strict";
import { DesktopBrowser } from "../browser.mjs";

test("browser bridge preserves structure and remaps secret and action scopes", async () => {
  let staged, dispatched;
  const workspace = {
    pages: new Map([["page", {}]]),
    secrets: new Map([["token", "secret"]]),
    observe: async () => ({
      status: "Observed",
      nodes: [
        {
          id: 3,
          version: 1,
          role: "root",
          children: [10],
          label: "Browser",
          actions: [],
        },
        {
          id: 10,
          version: 7,
          role: "text",
          kind: "password",
          value: "",
          children: [],
          label: "Password",
          actions: ["SetValue"],
        },
      ],
    }),
    stage: (scope, secret) => {
      staged = { scope, secret };
      return "opaque-token";
    },
    dispatch: async (request) => {
      dispatched = request;
      return { status: "dispatched" };
    },
  };
  const bridge = new DesktopBrowser(workspace);
  const capture = await bridge.request({ op: "capture" });
  assert.equal(capture.nodes[0].role, "group");
  assert.deepEqual(capture.nodes[1].parent, capture.nodes[0].key);
  assert.equal(capture.nodes[1].fingerprint, "7");
  const binding = { key: capture.nodes[1].key, fingerprint: "7" };
  const scope = {
    target: 900,
    version: 12,
    client: "client",
    epoch: "epoch",
    lease: "lease",
    generation: 3,
  };
  bridge.stage(scope, "private", binding);
  assert.deepEqual(staged.scope, { ...scope, target: 10, version: 7 });
  const result = await bridge.request({
    ...scope,
    ...binding,
    op: "dispatch",
    action: "SetValue",
    value: "opaque-token",
    expires: Date.now() + 2000,
  });
  assert.equal(result.status, "dispatched");
  assert.ok(result.capture);
  assert.equal(dispatched.target, 10);
  assert.equal(dispatched.version, 7);
  for (const k of ["client", "epoch", "lease", "generation"])
    assert.equal(dispatched[k], scope[k]);
  assert.equal(dispatched.value, "opaque-token");
  bridge.clearSecrets();
  assert.equal(workspace.secrets.size, 0);
});

test("desktop browser approvals isolate public IDs and retire password scopes on unshare", async () => {
  const { DesktopWorkspace } = await import("../workspace.mjs");
  let dispatched, staged;
  const workspace = {
    pages: new Map(),
    secrets: new Map(),
    closed: false,
    observe: async () => ({
      status: "Observed",
      nodes: [
        {
          id: 3,
          version: 1,
          role: "root",
          label: "Browser",
          children: [10, 11, 12],
          actions: [],
        },
        {
          id: 10,
          version: 7,
          role: "text",
          kind: "password",
          value: "",
          label: "Password",
          children: [],
          actions: ["SetValue"],
        },
        {
          id: 11,
          version: 2,
          role: "checkbox",
          value: true,
          label: "Reviewed",
          children: [],
          actions: ["SetValue"],
        },
      ],
    }),
    stage: (scope, value) => {
      staged = scope;
      workspace.secrets.set("token", value);
      return "token";
    },
    dispatch: async (r) => {
      dispatched = r;
      return { status: "dispatched" };
    },
  };
  const originalCapture = workspace.observe;
  workspace.observe = async () => {
    const c = await originalCapture();
    c.nodes.push(
      {
        id: 12,
        version: 1,
        role: "group",
        kind: "view:list",
        label: "",
        children: [13],
        actions: [],
      },
      {
        id: 13,
        version: 1,
        role: "label",
        label: "List item",
        children: [],
        actions: [],
      },
    );
    return c;
  };
  const bridge = new DesktopBrowser(workspace);
  const desktop = new DesktopWorkspace(async (r) => {
    assert.equal(r.op, "list", "browser targets must never reach AT-SPI");
    return { apps: [] };
  }, bridge);
  await desktop.local({ op: "apps" });
  assert.equal(
    desktop.publication.nodes.some((n) => n.label === "Password"),
    false,
  );
  await desktop.local({ op: "share", id: "browser" });
  assert.ok(
    desktop.publication.nodes.some(
      (n) => n.kind === "view:list" && n.children.length === 1,
    ),
  );
  const field = desktop.publication.nodes.find((n) => n.label === "Password");
  assert.notEqual(field.id, 10);
  assert.equal(
    desktop.publication.nodes.find((n) => n.label === "Reviewed").value,
    true,
  );
  const scope = {
    target: field.id,
    version: field.version,
    client: "c",
    lease: "l",
    generation: 3,
    epoch: "e",
  };
  assert.equal(desktop.stage(scope, "private"), "token");
  assert.equal(staged.target, 10);
  assert.equal(staged.version, 7);
  assert.equal(
    (
      await desktop.dispatch({
        ...scope,
        action: "SetValue",
        value: "token",
        deadlineMs: 2000,
      })
    ).status,
    "dispatched",
  );
  assert.equal(dispatched.target, 10);
  assert.equal(dispatched.client, "c");
  if (desktop.busy) await desktop.busy;
  await desktop.local({ op: "unshare", id: "browser" });
  assert.equal(workspace.secrets.size, 0);
  assert.throws(() => desktop.stage(scope, "private"));
  assert.equal(
    (
      await desktop.dispatch({
        ...scope,
        action: "SetValue",
        value: "token",
        deadlineMs: 2000,
      })
    ).status,
    "rejected",
  );
});
