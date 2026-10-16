import { test } from "node:test";
import assert from "node:assert/strict";
import { SemanticNodes } from "../semantic-nodes.mjs";
test("stable IDs and versions cover content and private dispatch guards", () => {
  const first = new SemanticNodes();
  const original = first.node(
    "button",
    { label: "Save" },
    { key: "private" },
    "a",
  );
  const next = new SemanticNodes(first);
  assert.deepEqual(
    next.node("button", { label: "Save" }, { key: "private" }, "a"),
    original,
  );
  assert.equal(next.node("button", { label: "Save" }, {}, "b").version, 2);
  assert.equal(next.node("button", { label: "Saved" }, {}, "b").version, 3);
  assert.equal(first.versions.get(original.id).version, 1);
  assert.equal(next.node("root", { role: "root" }).id, 3);
});
test("pruning retires targets and identities without reusing discarded IDs", () => {
  const nodes = new SemanticNodes();
  const old = nodes.node("button", {}, {});
  const abandoned = new SemanticNodes(nodes).id("abandoned");
  nodes.retain([]);
  assert.equal(nodes.keys.size + nodes.versions.size + nodes.targets.size, 0);
  assert.ok(nodes.node("button", {}).id > abandoned);
  assert.notEqual(nodes.id("button"), old.id);
});
test("unchanged identities still work when the identity limit is reached", () => {
  const nodes = new SemanticNodes();
  const id = nodes.id("existing");
  nodes.sequence.next = 1_000_000;
  assert.equal(nodes.id("existing"), id);
  assert.throws(() => nodes.id("new"), /identity limit/);
});
