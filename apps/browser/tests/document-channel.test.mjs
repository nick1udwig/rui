import test from "node:test";
import assert from "node:assert/strict";
import { DocumentPublisher, DocumentReceiver } from "../src/document-channel.mjs";

test("document patches preserve unchanged records and reject gaps atomically", () => {
  const publisher = new DocumentPublisher(), receiver = new DocumentReceiver();
  const first = { generation: "a".repeat(32), revision: 1, root: { id: 1, tag: "p", children: [{ id: 2, text: "before" }] }, styles: [] };
  receiver.apply(publisher.publish(first));
  const next = structuredClone(first);
  next.revision = 2;
  next.root.children[0].text = "after";
  const patch = publisher.publish(next);
  assert.equal(patch.put.length, 1);
  assert.throws(() => receiver.apply({ ...patch, base: 99 }), /checkpoint/);
  assert.equal(receiver.revision, 1);
  assert.equal(receiver.apply(patch).root.children[0].text, "after");
  assert.throws(() => receiver.apply({ ...patch, base: 2, revision: 3, put: [{ id: 1, children: [1] }] }), /graph/);
  assert.equal(receiver.revision, 2);
});
