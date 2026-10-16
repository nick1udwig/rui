import { Workspace } from "./workspace.mjs";
import { createServer as httpServer } from "node:http";
import { createServer as httpsServer } from "node:https";
import { createServer as socketServer, createConnection } from "node:net";
import { readFile, stat, chmod } from "node:fs/promises";
import { resolve, extname } from "node:path";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { WebSocket, WebSocketServer } from "ws";
import QRCode from "qrcode";

const directory = process.env.RUI_STATE,
  origin = process.env.RUI_ORIGIN;
const publicPath = process.env.RUI_PUBLIC_PATH || "/";
const webRoot = resolve(import.meta.dirname, "dist");
const desktop = process.env.RUI_WORKSPACE === "desktop";
const workspace = desktop
  ? await (await import("../desktop/workspace.mjs")).DesktopWorkspace.launch()
  : await Workspace.launch(directory);
let closing = false;
let invitation = randomBytes(32).toString("hex"),
  invitationDeadline = Date.now() + 300_000;
async function printInvitation() {
  console.error(
    `\nOpen on your phone: ${origin}${publicPath}#invite=${invitation}\nPairing invitation expires in 5 minutes. Approve the phone's code in this terminal.`,
  );
  console.error(
    await QRCode.toString(`${origin}${publicPath}#invite=${invitation}`, {
      type: "terminal",
      small: true,
    }),
  );
}
const pairings = new Map();
const equal = (a, b) =>
  typeof a === "string" &&
  a.length === b.length &&
  timingSafeEqual(Buffer.from(a), Buffer.from(b));
function admin(request) {
  return new Promise((resolve, reject) => {
    const socket = createConnection(resolvePath("admin.sock"));
    let data = "";
    socket.setTimeout(6000, () => socket.destroy(Error("Host deadline")));
    socket.on("connect", () => socket.write(JSON.stringify(request) + "\n"));
    socket.on("data", (chunk) => {
      data += chunk;
      if (data.length > 8192) socket.destroy(Error("Host response limit"));
      else if (data.includes("\n")) {
        socket.end();
        try {
          const result = JSON.parse(data);
          if (result.error) reject(Error(result.error));
          else resolve(result);
        } catch (e) {
          reject(e);
        }
      }
    });
    socket.on("error", reject);
  });
}
function resolvePath(name) {
  return resolve(directory, name);
}
async function body(request) {
  let data = "";
  for await (const chunk of request) {
    data += chunk;
    if (Buffer.byteLength(data) > 65536) throw Error("Request limit");
  }
  return JSON.parse(data);
}
function respond(response, status, data) {
  response.writeHead(status, {
    "Content-Type": "application/json",
    "Cache-Control": "no-store",
  });
  response.end(JSON.stringify(data));
}
function credentials(request) {
  const credential = request.headers.authorization?.replace(/^Bearer /, ""),
    client = request.headers["x-rui-client"];
  if (
    !/^[a-f0-9]{64}$/.test(credential || "") ||
    !/^[a-f0-9]{32}$/.test(client || "")
  )
    throw Error("Device authentication required");
  return { credential, client };
}
const handler = async (request, response) => {
  try {
    response.setHeader("X-Content-Type-Options", "nosniff");
    response.setHeader("Referrer-Policy", "no-referrer");
    response.setHeader(
      "Content-Security-Policy",
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'none'",
    );
    if (request.headers.host !== new URL(origin).host)
      return respond(response, 403, { error: "Host rejected" });
    const url = new URL(request.url, origin);
    if (url.pathname.startsWith("/api/")) {
      if (
        request.method !== "POST" ||
        request.headers.origin !== origin ||
        request.headers["content-type"] !== "application/json"
      )
        return respond(response, 403, { error: "Origin rejected" });
      const data = await body(request);
      if (url.pathname === "/api/pair") {
        for (const [ticket, pending] of pairings)
          if (pending.expires < Date.now()) pairings.delete(ticket);
        if (
          !equal(data.invitation, invitation) ||
          Date.now() > invitationDeadline ||
          pairings.size >= 16 ||
          !/^[a-f0-9]{32}$/.test(data.client || "")
        )
          throw Error("Pairing invitation expired or invalid");
        const result = await admin({ op: "pair", client: data.client });
        const ticket = randomBytes(32).toString("hex");
        pairings.set(ticket, {
          client: data.client,
          code: result.code,
          expires: Date.now() + 120_000,
        });
        return respond(response, 200, { ...result, ticket });
      }
      if (url.pathname === "/api/pair/poll") {
        const pending = pairings.get(data.ticket);
        if (
          !pending ||
          pending.expires < Date.now() ||
          pending.client !== data.client
        )
          throw Error("Pairing expired");
        const result = await admin({
          op: "poll",
          code: pending.code,
          client: pending.client,
        });
        if (result.credential) pairings.delete(data.ticket);
        return respond(response, 200, result);
      }
      if (url.pathname === "/api/secret") {
        const auth = credentials(request);
        const scope = await admin({
          op: "secret",
          ...auth,
          target: data.target,
        });
        return respond(response, 200, {
          token: workspace.stage(scope, data.secret),
        });
      }
      return respond(response, 404, { error: "Unknown operation" });
    }
    if (request.method !== "GET" && request.method !== "HEAD")
      return respond(response, 405, { error: "Method rejected" });
    const path = resolve(
      webRoot,
      "." +
        decodeURIComponent(url.pathname === "/" ? "/index.html" : url.pathname),
    );
    if (
      !path.startsWith(webRoot + "/") ||
      !(await stat(path).catch(() => null))?.isFile()
    )
      return respond(response, 404, { error: "Not found" });
    response.writeHead(200, {
      "Content-Type":
        {
          ".html": "text/html",
          ".js": "text/javascript",
          ".css": "text/css",
          ".wasm": "application/wasm",
          ".svg": "image/svg+xml",
        }[extname(path)] || "application/octet-stream",
      "Cache-Control": "no-cache",
    });
    const content =
      request.method === "HEAD" ? undefined : await readFile(path);
    // Vite emits relative assets. Supply the external mount even when Serve
    // strips it, and when the browser opens /bart without its trailing slash.
    response.end(
      extname(path) === ".html" && content
        ? content
            .toString("utf8")
            .replace('<base href="/" />', `<base href="${publicPath}" />`)
            .replace(
              'data-workspace="browser"',
              `data-workspace="${desktop ? "desktop" : "browser"}"`,
            )
            .replaceAll("RUI Browser", desktop ? "RUI Desktop" : "RUI Browser")
        : content,
    );
  } catch {
    respond(response, 400, {
      error: "Request rejected. Check pairing, control, and the current page.",
    });
  }
};
const server = process.env.RUI_WEB_CERT
  ? httpsServer(
      {
        cert: await readFile(process.env.RUI_WEB_CERT),
        key: await readFile(process.env.RUI_WEB_KEY),
      },
      handler,
    )
  : httpServer(handler);
server.requestTimeout = 10_000;
server.headersTimeout = 5000;
server.maxConnections = 32;
const wss = new WebSocketServer({
  noServer: true,
  maxPayload: 1024 * 1024,
  perMessageDeflate: false,
});
server.on("upgrade", async (request, socket, head) => {
  if (
    request.url !== "/rui" ||
    request.headers.origin !== origin ||
    request.headers.host !== new URL(origin).host ||
    request.headers["sec-websocket-protocol"] !== "rui.v1"
  )
    return socket.destroy();
  try {
    const { port } = JSON.parse(await readFile(resolvePath("backend.json")));
    const upstream = new WebSocket(`wss://127.0.0.1:${port}/rui`, "rui.v1", {
      ca: await readFile(resolvePath("host.pem")),
      servername: "rui-browser",
      rejectUnauthorized: true,
      origin,
      perMessageDeflate: false,
      maxPayload: 1024 * 1024,
      handshakeTimeout: 5000,
    });
    upstream.once("error", (error) => {
      console.error("RUI backend connection:", error.message);
      socket.destroy();
    });
    upstream.once("open", () =>
      wss.handleUpgrade(request, socket, head, (downstream) => {
        const forward = (destination, source) => (data, binary) => {
          if (
            !binary ||
            destination.readyState !== WebSocket.OPEN ||
            destination.bufferedAmount + data.length > 2 * 1024 * 1024
          ) {
            source.close(1008);
            destination.close(1008);
            return;
          }
          destination.send(data, { binary: true });
        };
        downstream.on("message", forward(upstream, downstream));
        upstream.on("message", forward(downstream, upstream));
        downstream.on("close", () => upstream.close());
        upstream.on("close", () => downstream.close());
        downstream.on("error", () => upstream.close());
        upstream.on("error", () => downstream.close());
      }),
    );
    socket.on("close", () => upstream.close());
  } catch {
    socket.destroy();
  }
});
await new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(
    Number(process.env.RUI_WEB_PORT),
    process.env.RUI_WEB_LISTEN || "127.0.0.1",
    resolve,
  );
});

// Only the owner can reach this socket in the mode-0700 state directory.
let owner;
const rpc = socketServer((socket) => {
  if (owner) return socket.destroy();
  owner = socket;
  let pending = Buffer.alloc(0),
    processing = false;
  socket.on("data", async (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    if (pending.length > 65540) return socket.destroy();
    if (processing || pending.length < 4) return;
    const size = pending.readUInt32BE(0);
    if (size > 65536) return socket.destroy();
    if (pending.length < size + 4) return;
    processing = true;
    const bytes = pending.subarray(4, size + 4);
    pending = pending.subarray(size + 4);
    try {
      const request = JSON.parse(bytes);
      let result;
      if (request.op === "invite") {
        invitation = randomBytes(32).toString("hex");
        invitationDeadline = Date.now() + 300_000;
        await printInvitation();
        result = { ok: true };
      } else if (
        ["apps", "share", "unshare", "diagnose", "protect", "session"].includes(request.op)
      ) {
        try {
          result = await workspace.local(request);
        } catch (error) {
          result = { error: error.message };
        }
      } else
        result =
          request.op === "capture"
            ? await workspace.capture()
            : request.op === "dispatch"
              ? await workspace.dispatch(request)
              : { error: "Unsupported operation" };
      const data = Buffer.from(JSON.stringify(result));
      if (data.length > 1024 * 1024) throw Error("Response limit");
      const header = Buffer.alloc(4);
      header.writeUInt32BE(data.length);
      socket.write(Buffer.concat([header, data]));
    } catch {
      socket.destroy();
    } finally {
      processing = false;
    }
  });
  socket.on("close", () => {
    void shutdown();
  });
  socket.on("error", () => {});
});
await new Promise((resolve) => rpc.listen(resolvePath("worker.sock"), resolve));
await chmod(resolvePath("worker.sock"), 0o600);
// The invitation promises a usable service, including the Rust backend.
const backendDeadline = Date.now() + 30000;
while (true) {
  try {
    JSON.parse(await readFile(resolvePath("backend.json")));
    break;
  } catch {
    if (Date.now() >= backendDeadline)
      throw Error("RUI backend startup deadline");
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
}
await printInvitation();
async function shutdown() {
  if (closing) return;
  closing = true;
  server.close();
  rpc.close();
  for (const client of wss.clients) client.terminate();
  await workspace.context.close().catch(() => {});
  process.exit(0);
}
process.on("SIGTERM", shutdown);
process.on("SIGINT", shutdown);
