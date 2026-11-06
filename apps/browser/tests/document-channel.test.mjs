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

test("resource bodies stay out of checkpoints and unchanged styles stay out of patches", () => {
  const publisher = new DocumentPublisher(), receiver = new DocumentReceiver();
  const key = 'b'.repeat(64), style = 'c'.repeat(64);
  const child = { generation: 'd'.repeat(32), revision: 1, root: { id: 1, tag: 'img', children: [] }, resources: { [key]: 'image-body' }, styleResources: {}, styles: [] };
  const first = { generation: 'a'.repeat(32), revision: 1, root: { id: 1, tag: 'iframe', document: child, children: [] }, resources: { [key]: 'image-body' }, styleResources: { [style]: 'body{color:red}' }, styles: [{ resource: style }] };
  const packet = publisher.publish(first);
  assert.ok(!JSON.stringify(packet).includes('image-body'));
  assert.ok(!JSON.stringify(packet).includes('body{color:red}'));
  const snapshot = receiver.apply(packet);
  assert.deepEqual(snapshot.resources[key], { bytes: 10 });
  snapshot.root.document.resources[key] = 'hydrated';
  const patch = publisher.publish({ ...first, revision: 2 });
  assert.equal(patch.styles, undefined);
  assert.deepEqual(patch.resources, {});
  assert.deepEqual(patch.styleResources, {});
  assert.deepEqual(receiver.apply(patch).root.document.resources[key], { bytes: 10 });
});
