import { test } from "node:test";
import assert from "node:assert/strict";
import { indexTree } from "../tree.mjs";
const key = (name) => ["app", name];
const node = (name, children = [], parent) => ({
  key: key(name),
  children: children.map(key),
  actions: [],
  parent,
});
test("indexes valid trees and infers omitted parent links", () => {
  const nodes = [node("root", ["child"]), node("child", [], key("root"))];
  const tree = indexTree(nodes);
  assert.equal(tree.byKey.get(tree.keyOf(key("child"))), nodes[1]);
  assert.equal(tree.parentOf(nodes[1]), nodes[0]);
});
for (const [name, nodes] of [
  ["empty", []],
  ["duplicate", [node("root"), node("root")]],
  ["missing child", [node("root", ["missing"])]],
  ["duplicate child", [node("root", ["child", "child"]), node("child")]],
  ["wrong parent", [node("root", ["child"]), node("child", [], key("other"))]],
  ["cycle", [node("root", ["child"]), node("child", ["root"])]],
  ["disconnected cycle", [node("root"), node("a", ["b"]), node("b", ["a"])]],
  [
    "depth",
    Array.from({ length: 66 }, (_, i) =>
      node(String(i), i < 65 ? [String(i + 1)] : []),
    ),
  ],
  ["size", Array.from({ length: 1025 }, (_, i) => node(String(i)))],
])
  test(`rejects ${name}`, () => assert.throws(() => indexTree(nodes)));
