import test from "node:test";
import assert from "node:assert/strict";
import { Readable } from "node:stream";
import { createServer } from "node:net";
import { readJson, requestJson } from "../json-stream.mjs";

test("JSON reads preserve Unicode at every byte boundary", async () => {
  const value = { text: "café 東京 👋" };
  const bytes = Buffer.from(JSON.stringify(value));
  for (let i = 1; i < bytes.length; i++)
    assert.deepEqual(
      await readJson(Readable.from([bytes.subarray(0, i), bytes.subarray(i)])),
      value,
    );
  assert.deepEqual(
    await readJson(Readable.from([...bytes, 10].map((b) => Buffer.from([b]))), {
      line: true,
    }),
    value,
  );
});

test("JSON limits count bytes and truncated replies reject", async () => {
  const bytes = Buffer.from('"東京"');
  await assert.rejects(
    readJson(Readable.from([bytes]), { limit: bytes.length - 1 }),
    /limit/,
  );
  await assert.rejects(
    readJson(Readable.from(['{"ok":true}']), { line: true }),
    /disconnected/,
  );
  await assert.rejects(readJson(Readable.from(["not json"])));
});

test("local RPC settles on success, host error, truncation and timeout", async () => {
  const server = createServer((socket) => {
    socket.once("data", (data) => {
      const { op } = JSON.parse(data);
      if (op === "ok") socket.end('{"text":"東京"}\n');
      if (op === "error") socket.end('{"error":"rejected"}\n');
      if (op === "truncated") socket.end('{"ok":');
    });
    socket.on("error", () => {});
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = { port: server.address().port, host: "127.0.0.1" };
  try {
    assert.deepEqual(await requestJson(address, { op: "ok" }), {
      text: "東京",
    });
    await assert.rejects(requestJson(address, { op: "error" }), /rejected/);
    await assert.rejects(
      requestJson(address, { op: "truncated" }),
      /disconnected/,
    );
    await assert.rejects(
      requestJson(address, { op: "silent" }, { timeout: 30 }),
      /deadline/,
    );
  } finally {
    server.close();
  }
});
