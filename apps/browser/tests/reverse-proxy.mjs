// Local TLS-terminating proxy fixture: preserve Host/Origin as Tailscale Serve does.
import { createServer } from "node:https";
import { request } from "node:http";
import { createConnection } from "node:net";
import { readFile } from "node:fs/promises";

export async function reverseProxy({
  cert,
  key,
  port,
  backendPort,
  mount = "",
}) {
  const requests = [];
  const routed = (incoming, upgrade = false) => {
    const url = new URL(incoming.url, "https://localhost");
    requests.push({ path: url.pathname, upgrade });
    if (
      mount &&
      url.pathname !== mount &&
      !url.pathname.startsWith(mount + "/")
    )
      return null;
    return (url.pathname.slice(mount.length) || "/") + url.search;
  };
  const server = createServer(
    { cert: await readFile(cert), key: await readFile(key) },
    (incoming, outgoing) => {
      const path = routed(incoming);
      if (path === null) {
        outgoing.writeHead(404);
        outgoing.end();
        return;
      }
      const upstream = request(
        {
          host: "127.0.0.1",
          port: backendPort,
          path,
          method: incoming.method,
          headers: incoming.headers,
        },
        (response) => {
          outgoing.writeHead(response.statusCode, response.headers);
          response.pipe(outgoing);
        },
      );
      upstream.on("error", () => {
        outgoing.writeHead(502);
        outgoing.end();
      });
      incoming.pipe(upstream);
    },
  );
  const sockets = new Set();
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  server.on("upgrade", (incoming, socket, head) => {
    const path = routed(incoming, true);
    if (path === null) return socket.destroy();
    const upstream = createConnection(
      { host: "127.0.0.1", port: backendPort },
      () => {
        const headers = [];
        for (let i = 0; i < incoming.rawHeaders.length; i += 2)
          headers.push(
            `${incoming.rawHeaders[i]}: ${incoming.rawHeaders[i + 1]}`,
          );
        upstream.write(
          `${incoming.method} ${path} HTTP/1.1\r\n${headers.join("\r\n")}\r\n\r\n`,
        );
        if (head.length) upstream.write(head);
        socket.pipe(upstream).pipe(socket);
      },
    );
    socket.on("error", () => upstream.destroy());
    upstream.on("error", () => socket.destroy());
    socket.on("close", () => upstream.destroy());
    upstream.on("close", () => socket.destroy());
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  return {
    requests,
    close: async () => {
      const closed = new Promise((resolve) => server.close(resolve));
      for (const socket of sockets) socket.destroy();
      await closed;
    },
  };
}
