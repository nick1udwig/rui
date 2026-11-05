import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { GameSource } from "../model.mjs";
function fixture(t) {
  const path = mkdtempSync(join(tmpdir(), "rui-game-model-"));
  t.after(() => rmSync(path, { recursive: true, force: true }));
  const source = new GameSource(path);
  const frame = (sequence, lines = [], extra = {}) =>
    writeFileSync(
      join(path, "frame.json"),
      JSON.stringify({ sequence, lines, ...extra }),
    );
  return { path, source, frame };
}
test("line slots survive disappearance and malformed geometry rejects", (t) => {
  const { source, frame } = fixture(t);
  frame(1, [[0, 0, 10, 10, 1, 1, 1]]);
  assert.equal(source.capture().vectors.slots, 1);
  frame(2);
  assert.equal(source.capture().vectors.slots, 1);
  frame(3, [[0, 0, 10, 10, 2, 0, 0]]);
  assert.throws(() => source.capture(), /vector bounds/);
  frame(4, new Array(2049).fill([0, 0, 1, 1, 1, 1, 1]));
  assert.throws(() => source.capture(), /frame bounds/);
});
test("only a current native acknowledgement confirms input", async (t) => {
  const { source, frame } = fixture(t);
  frame(1);
  const request = {
    target: 4,
    version: 1,
    action: "SetValue",
    value: true,
    deadlineMs: 100,
    epoch: "e",
    client: "c",
    lease: "l",
    generation: 1,
  };
  const result = source.dispatch(request);
  frame(2, [], { inputRevision: source.inputRevision, mask: 1 });
  assert.equal((await result).status, "dispatched");
  assert.equal(source.mask, 1);
  assert.equal((await source.dispatch(request)).status, "rejected");
  source.lastInput = 0;
  source.watchdog();
  assert.equal(source.mask, 0);
});
test("lost acknowledgement releases inputs and reports unknown", async (t) => {
  const { source, frame, path } = fixture(t);
  frame(1);
  const result = await source.dispatch({
    target: 4,
    version: 1,
    action: "SetValue",
    value: true,
    deadlineMs: 12,
    epoch: "e",
    client: "c",
    lease: "l",
    generation: 1,
  });
  assert.equal(result.status, "unknown");
  assert.equal(source.mask, 0);
  assert.equal(readFileSync(join(path, "inputs"), "utf8").split(" ")[1], "0");
});
test("a new lease clears the preceding owner’s held inputs", async (t) => {
  const { source, frame } = fixture(t);
  frame(1);
  const request = {
    target: 4,
    version: 1,
    action: "SetValue",
    value: true,
    deadlineMs: 50,
    epoch: "e",
    client: "c",
    lease: "a",
    generation: 1,
  };
  let result = source.dispatch(request);
  frame(2, [], { inputRevision: source.inputRevision, mask: 1 });
  await result;
  result = source.dispatch({
    ...request,
    target: 5,
    lease: "b",
    generation: 2,
  });
  frame(3, [], { inputRevision: source.inputRevision, mask: 2 });
  assert.equal((await result).status, "dispatched");
  assert.equal(source.mask, 2);
});
