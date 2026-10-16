import assert from "node:assert/strict";
import { test } from "node:test";
import { createServer, createConnection } from "node:net";
import { once } from "node:events";
import { serveFrames } from "../framed-rpc.mjs";
const frame = (value) => {
  const body = Buffer.from(JSON.stringify(value)),
    head = Buffer.alloc(4);
  head.writeUInt32BE(body.length);
  return Buffer.concat([head, body]);
};
async function connection(t, handle) {
  const server = createServer((socket) => {
    socket.on("error", () => {});
    void serveFrames(socket, handle);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const socket = createConnection(server.address().port, "127.0.0.1");
  socket.on("error", () => {});
  t.after(() => {
    socket.destroy();
    server.close();
  });
  await once(socket, "connect");
  return socket;
}
test(
  "fragmented and coalesced requests drain in order across an awaited handler",
  { timeout: 3000 },
  async (t) => {
    let release, entered;
    const gate = new Promise((r) => (release = r)),
      started = new Promise((r) => (entered = r));
    const seen = [];
    const socket = await connection(t, async (n) => {
      seen.push(n);
      if (n === 1) {
        entered();
        await gate;
      }
      return n;
    });
    let received = Buffer.alloc(0);
    const done = new Promise((resolve) =>
      socket.on("data", (data) => {
        received = Buffer.concat([received, data]);
        if (received.length === 15) resolve();
      }),
    );
    const first = frame(1);
    socket.write(first.subarray(0, 2));
    socket.write(Buffer.concat([first.subarray(2), frame(2)]));
    await started;
    socket.write(frame(3));
    release();
    await done;
    assert.deepEqual(seen, [1, 2, 3]);
    assert.deepEqual(received, Buffer.concat([frame(1), frame(2), frame(3)]));
  },
);
for (const [name, bytes] of [
  ["oversized", Buffer.from([0, 1, 0, 1])],
  ["truncated", Buffer.from([0, 0, 0, 4, 123])],
  ["invalid JSON", Buffer.from([0, 0, 0, 1, 123])],
]) {
  test(
    `${name} frames close without dispatch`,
    { timeout: 3000 },
    async (t) => {
      const socket = await connection(t, () =>
        assert.fail("must not dispatch"),
      );
      const closed = once(socket, "close");
      socket.end(bytes);
      await closed;
    },
  );
}
