import { readFileSync } from "node:fs";
import { Agent } from "node:https";
import { resolve } from "node:path";
export function demoConfig(root: string, command: string) {
  if (command === "build") return {};
  // Forward exact binary packets and the original Origin. No credential/key is
  // delivered to JavaScript. The RUI host authenticates packets independently.
  const directory = process.env.RUI_DEMO_IDENTITY ?? "/tmp/rui-browser-demo";
  const proxy = {
    "/rui": {
      target: process.env.RUI_HOST_URL ?? "wss://127.0.0.1:8443",
      ws: true,
      secure: true,
      agent: new Agent({
        ca: readFileSync(resolve(directory, "host.pem")),
        servername: "rui-demo",
        rejectUnauthorized: true,
      }),
    },
  };
  const cert = process.env.RUI_WEB_CERT,
    key = process.env.RUI_WEB_KEY;
  if (Boolean(cert) !== Boolean(key))
    throw new Error("set both RUI_WEB_CERT and RUI_WEB_KEY");
  const https =
    cert && key
      ? { cert: readFileSync(cert), key: readFileSync(key) }
      : undefined;
  const allowedHosts = (process.env.RUI_WEB_HOSTS ?? "localhost").split(",");
  return {
    server: {
      host: "0.0.0.0",
      port: 5173,
      strictPort: true,
      https,
      allowedHosts,
      proxy,
      fs: {
        allow: [
          resolve(root),
          resolve(root, "../../client-browser"),
        ],
      },
    },
    preview: {
      host: "0.0.0.0",
      port: 5173,
      strictPort: true,
      https,
      allowedHosts,
      proxy,
    },
  };
}
