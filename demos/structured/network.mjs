// Isolated replay fixture: real TLS/WebSocket bytes and real WASM acknowledgements.
// This is not an authenticated production host or an input-to-photon benchmark.
import assert from "node:assert/strict";
import { createServer as httpsServer } from "node:https";
import { createServer as tcpServer, connect as tcpConnect } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { createHash, randomBytes } from "node:crypto";
import { readFile, writeFile, mkdtemp, rm } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import init, { Session } from "../../client-browser/wasm/rui_client_wasm.js";
const [input, output, delayArg = "0"] = process.argv.slice(2);
assert(
  input && output,
  "usage: network.mjs SESSION_PACKETS OUTPUT_JSON [MESSAGE_DELAY_MS]",
);
const delay = Number(delayArg);
assert(Number.isFinite(delay) && delay >= 0 && delay <= 1000);
function packets(b) {
  const a = [];
  for (let i = 0; i < b.length;) {
    assert(i + 4 <= b.length);
    let n = b.readUInt32BE(i);
    i += 4;
    assert(n > 0 && i + n <= b.length);
    a.push(b.subarray(i, i + n));
    i += n;
  }
  return a;
}
function kind(b) {
  let i = 0;
  const v = () => {
    let n = 0,
      s = 0,
      x;
    do {
      assert(i < b.length && s < 56);
      x = b[i++];
      n += (x & 127) * 2 ** s;
      s += 7;
    } while (x & 128);
    return n;
  };
  while (i < b.length) {
    let k = v(),
      t = k >>> 3,
      w = k & 7;
    if (w === 0) {
      let n = v();
      if (t === 5) return n;
    } else if (w === 2) {
      let n = v();
      i += n;
    } else if (w === 1) i += 8;
    else if (w === 5) i += 4;
    else throw Error("wire type");
  }
  throw Error("packet kind");
}
function frame(data, mask = false) {
  let extra = data.length < 126 ? 0 : data.length <= 65535 ? 2 : 8;
  let b = Buffer.alloc(2 + extra + (mask ? 4 : 0) + data.length);
  b[0] = 0x82;
  b[1] =
    (mask ? 128 : 0) | (extra === 0 ? data.length : extra === 2 ? 126 : 127);
  if (extra === 2) b.writeUInt16BE(data.length, 2);
  if (extra === 8) b.writeBigUInt64BE(BigInt(data.length), 2);
  let at = 2 + extra;
  let key = mask ? randomBytes(4) : null;
  if (key) {
    key.copy(b, at);
    at += 4;
  }
  for (let i = 0; i < data.length; i++)
    b[at + i] = data[i] ^ (key ? key[i % 4] : 0);
  return b;
}
function parser(callback) {
  let pending = Buffer.alloc(0);
  return (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    assert(pending.length < 2 ** 22);
    while (pending.length >= 2) {
      assert(
        (pending[0] & 0x8f) === 0x82,
        "only complete binary fixture frames",
      );
      let n = pending[1] & 127,
        at = 2;
      if (n === 126) {
        if (pending.length < 4) return;
        n = pending.readUInt16BE(2);
        at = 4;
      }
      if (n === 127) {
        if (pending.length < 10) return;
        n = Number(pending.readBigUInt64BE(2));
        at = 10;
      }
      assert(n <= 2 ** 20);
      const masked = !!(pending[1] & 128);
      if (pending.length < at + (masked ? 4 : 0) + n) return;
      const key = masked ? pending.subarray(at, at + 4) : null;
      at += masked ? 4 : 0;
      const data = Buffer.from(pending.subarray(at, at + n));
      if (key) for (let i = 0; i < n; i++) data[i] ^= key[i % 4];
      pending = pending.subarray(at + n);
      callback(data);
    }
  };
}
const source = packets(await readFile(input));
assert(kind(source[0]) === 2);
const groups = [];
let current = [];
for (const p of source.slice(1)) {
  current.push(p);
  if (kind(p) === 13) {
    groups.push(current);
    current = [];
  }
}
assert(!current.length);
await init({
  module_or_path: await readFile(
    new URL(
      "../../client-browser/wasm/rui_client_wasm_bg.wasm",
      import.meta.url,
    ),
  ),
});
const temp = await mkdtemp(join(tmpdir(), "rui-wire-"));
let server, proxy, client, session, timeout;
const sockets = new Set();
try {
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      "1",
      "-subj",
      "/CN=localhost",
      "-addext",
      "subjectAltName=DNS:localhost",
      "-keyout",
      join(temp, "key.pem"),
      "-out",
      join(temp, "cert.pem"),
    ],
    { stdio: "ignore" },
  );
  const cert = await readFile(join(temp, "cert.pem")),
    key = await readFile(join(temp, "key.pem"));
  const samples = [];
  let incoming = 0,
    outgoing = 0,
    applicationDown = 0,
    applicationUp = 0,
    index = 0,
    sent = 0,
    cold = null;
  const started = performance.now();
  let finish, fail;
  const done = new Promise((resolve, reject) => {
    finish = resolve;
    fail = reject;
  });
  timeout = setTimeout(() => fail(Error("fixture timeout")), 120000);
  server = httpsServer({ cert, key }, (_, res) => {
    res.writeHead(404);
    res.end();
  });
  server.on("upgrade", (request, socket, head) => {
    socket.setNoDelay(true);
    const accept = createHash("sha1")
      .update(
        request.headers["sec-websocket-key"] +
          "258EAFA5-E914-47DA-95CA-C5AB0DC85B11",
      )
      .digest("base64");
    socket.write(
      "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " +
        accept +
        "\r\n\r\n",
    );
    function send(p) {
      applicationDown += p.length;
      socket.write(frame(p));
    }
    function next() {
      sent = performance.now();
      setTimeout(() => {
        for (const p of groups[index]) send(p);
      }, delay);
    }
    const parse = parser((p) => {
      applicationUp += p.length;
      const k = kind(p);
      if (k === 1) {
        send(source[0]);
        next();
      } else if (k === 30) {
        samples.push(performance.now() - sent);
      } else if (k === 31) {
        index++;
        if (index === 1)
          cold = {
            tlsDownBytes: outgoing,
            tlsUpBytes: incoming,
            elapsedMs: performance.now() - started,
          };
        if (index < groups.length) next();
        else finish();
      }
    });
    socket.on("data", (b) =>
      setTimeout(() => {
        try {
          parse(b);
        } catch (e) {
          fail(e);
        }
      }, delay),
    );
    if (head.length) parse(head);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  proxy = tcpServer((socket) => {
    socket.setNoDelay(true);
    const upstream = tcpConnect(server.address().port, "127.0.0.1");
    upstream.setNoDelay(true);
    sockets.add(socket);
    sockets.add(upstream);
    socket.on("data", (b) => {
      incoming += b.length;
      upstream.write(b);
    });
    upstream.on("data", (b) => {
      outgoing += b.length;
      socket.write(b);
    });
    socket.on("end", () => upstream.end());
    upstream.on("end", () => socket.end());
    socket.on("error", fail);
    upstream.on("error", fail);
  });
  await new Promise((r) => proxy.listen(0, "127.0.0.1", r));
  session = new Session(new Uint8Array(16).fill(3), "fixture");
  session.start(new Uint8Array(32).fill(7));
  let upgraded = false,
    header = Buffer.alloc(0),
    received = 0;
  let revision = 0n;
  const receive = parser((p) => {
    session.receive(p);
    if (session.revision !== revision) {
      revision = session.revision;
      session.presented(revision);
    }
    let ack;
    while ((ack = session.nextOutbound()))
      client.write(frame(Buffer.from(ack), true));
    received++;
  });
  client = tlsConnect(
    {
      host: "127.0.0.1",
      port: proxy.address().port,
      servername: "localhost",
      ca: cert,
    },
    () => {
      client.write(
        "GET /fixture HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: " +
          randomBytes(16).toString("base64") +
          "\r\n\r\n",
      );
    },
  );
  client.setNoDelay(true);
  client.on("error", fail);
  client.on("data", (b) => {
    try {
      if (!upgraded) {
        header = Buffer.concat([header, b]);
        const end = header.indexOf("\r\n\r\n");
        if (end < 0) return;
        assert(header.toString("ascii", 0, 12) === "HTTP/1.1 101");
        upgraded = true;
        let p;
        while ((p = session.nextOutbound()))
          client.write(frame(Buffer.from(p), true));
        b = header.subarray(end + 4);
      }
      receive(b);
    } catch (e) {
      fail(e);
    }
  });
  await done;
  clearTimeout(timeout);
  const peak = session.memoryPeak;
  session.close();
  assert.equal(session.memoryUsed, 0);
  session.free();
  session = null;
  client.end();
  await new Promise((r) => setTimeout(r, 30));
  for (const s of sockets) s.destroy();
  samples.sort((a, b) => a - b);
  const result = {
    publications: groups.length,
    receivedPackets: received,
    applicationDownBytes: applicationDown,
    applicationUpBytes: applicationUp,
    tlsDownBytes: outgoing,
    tlsUpBytes: incoming,
    cold,
    appliedRoundTripMsP50: samples[Math.ceil(samples.length * 0.5) - 1],
    appliedRoundTripMsP95: samples[Math.ceil(samples.length * 0.95) - 1],
    wasmAccountedPeakBytes: peak,
    messageDelayMs: delay,
    scope:
      "Actual loopback TLS stream including handshake, WebSocket upgrade/framing, WASM applied acknowledgements and simulated presented acknowledgements; excludes TCP/IP headers, browser assets, authorization service, rendering and input. Synthetic replay endpoint, not production host. Delays are per message delivery, not network emulation.",
  };
  await writeFile(output, JSON.stringify(result, null, 2) + "\n");
  console.log(JSON.stringify(result));
} finally {
  clearTimeout(timeout);
  for (const s of sockets) s.destroy();
  client?.destroy();
  session?.close();
  session?.free();
  server?.closeAllConnections();
  server?.close();
  proxy?.close();
  await rm(temp, { recursive: true, force: true });
}
