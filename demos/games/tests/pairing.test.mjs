import assert from "node:assert/strict";
import { createServer, request } from "node:http";
import { once } from "node:events";
import test from "node:test";
import { createPairing } from "../pairing.mjs";

const credential = "a1".repeat(32);
test("connection codes expire and rotation invalidates the old code", () => {
  let now = 0;
  const pairing = createPairing({
    credential,
    origin: "https://game.example",
    now: () => now,
  });
  assert.equal(pairing.redeem("000000000000").status, 403);
  const first = pairing.rotate();
  assert.match(first.code, /^[A-F0-9]{4}(-[A-F0-9]{4}){2}$/);
  assert.equal(new URL(first.url).hash, `#code=${first.code}`);
  assert(!first.url.includes(credential));
  assert.equal(
    pairing.redeem(first.code.toLowerCase().replaceAll("-", " ")).credential,
    credential,
  );
  assert.equal(pairing.redeem("é".repeat(12)).status, 403);
  const second = pairing.rotate();
  assert.equal(pairing.redeem(first.code).status, 403);
  assert.equal(pairing.redeem(second.code).status, 200);
  now = 300_000;
  assert.equal(pairing.redeem(second.code).status, 403);
});

test("guessing is globally bounded, including across clients", () => {
  let now = 0;
  const pairing = createPairing({
    credential,
    origin: "https://game.example",
    now: () => now,
  });
  const { code } = pairing.rotate();
  for (let i = 0; i < 20; i++) assert.equal(pairing.redeem("bad").status, 403);
  assert.equal(pairing.redeem(code).status, 429);
  now = 60_000;
  assert.equal(pairing.redeem(code).status, 200);
});

test("HTTP redemption requires the configured host, origin and JSON POST", async () => {
  let pairing;
  const server = createServer((req, res) =>
    pairing.middleware(req, res, () => {
      res.writeHead(404);
      res.end();
    }),
  );
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const origin = `http://127.0.0.1:${server.address().port}`;
  pairing = createPairing({ credential, origin });
  const { code } = pairing.rotate();
  const post = (
    headers = {},
    body = JSON.stringify({ code }),
    method = "POST",
  ) =>
    fetch(`${origin}/api/game/pair`, {
      method,
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        ...headers,
      },
      ...(method === "POST" ? { body } : {}),
    });
  try {
    assert.equal((await post({ Origin: "https://other.example" })).status, 403);
    const wrongHost = await new Promise((resolve, reject) => {
      const req = request(
        `${origin}/api/game/pair`,
        {
          method: "POST",
          headers: {
            Host: "other.example",
            Origin: origin,
            "Content-Type": "application/json",
          },
        },
        (res) => {
          res.resume();
          resolve(res.statusCode);
        },
      );
      req.on("error", reject);
      req.end(JSON.stringify({ code }));
    });
    assert.equal(wrongHost, 403);
    assert.equal((await post({ "Content-Type": "text/plain" })).status, 403);
    assert.equal((await post({}, undefined, "GET")).status, 403);
    assert.equal((await post({}, "{")).status, 400);
    assert.equal(
      (await post({}, JSON.stringify({ code: "x".repeat(1100) }))).status,
      413,
    );
    const response = await post();
    assert.equal(response.status, 200);
    assert.equal(response.headers.get("cache-control"), "no-store");
    assert.deepEqual(await response.json(), { credential });
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});
