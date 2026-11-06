import { randomBytes, timingSafeEqual } from "node:crypto";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";
import QRCode from "qrcode";

export function createPairing({ credential, origin, now = Date.now }) {
  if (!/^[a-f0-9]{64}$/.test(credential ?? ""))
    throw Error("Missing game authorization");
  let code,
    deadline,
    attempts = 0,
    windowStart = now();
  function rotate() {
    code = randomBytes(6).toString("hex").toUpperCase();
    deadline = now() + 300_000;
    attempts = 0;
    windowStart = now();
    const display = code.match(/.{4}/g).join("-");
    return { code: display, url: `${origin}/#code=${display}` };
  }
  function redeem(value) {
    if (now() - windowStart >= 60_000) {
      attempts = 0;
      windowStart = now();
    }
    if (++attempts > 20)
      return { status: 429, error: "Too many attempts. Wait one minute." };
    const normalized =
      typeof value === "string"
        ? value.replace(/[\s-]/g, "").toUpperCase()
        : "";
    if (
      !code ||
      now() >= deadline ||
      !/^[A-F0-9]{12}$/.test(normalized) ||
      !timingSafeEqual(Buffer.from(normalized), Buffer.from(code))
    )
      return {
        status: 403,
        error:
          "Code invalid or expired. Use the current code printed by the host.",
      };
    return { status: 200, credential };
  }
  async function middleware(request, response, next) {
    if (request.url?.split("?")[0] !== "/api/game/pair") return next();
    const reply = (status, data) => {
      response.writeHead(status, {
        "Content-Type": "application/json",
        "Cache-Control": "no-store",
        "Referrer-Policy": "no-referrer",
      });
      response.end(JSON.stringify(data));
    };
    if (
      request.method !== "POST" ||
      request.headers.origin !== origin ||
      request.headers.host !== new URL(origin).host ||
      request.headers["content-type"] !== "application/json"
    )
      return reply(403, { error: "Request rejected" });
    const timer = setTimeout(() => request.destroy(), 3000);
    timer.unref();
    try {
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        if (Buffer.byteLength(body) > 1024)
          return reply(413, { error: "Request too large" });
      }
      const { status, ...result } = redeem(JSON.parse(body).code);
      reply(status, result);
    } catch {
      if (!response.headersSent && !response.destroyed)
        reply(400, { error: "Invalid request" });
    } finally {
      clearTimeout(timer);
    }
  }
  return { rotate, redeem, middleware };
}

export async function printPairing(pairing, directory) {
  const { code, url } = pairing.rotate();
  const message = `Vector arcade: ${new URL(url).origin}\nConnection code: ${code}\nScan to connect: ${url}\nCode expires in 5 minutes. Type invite here for a new code.\n`;
  await writeFile(join(directory, "connection.txt"), message, { mode: 0o600 });
  await writeFile(
    join(directory, "connection.svg"),
    await QRCode.toString(url, { type: "svg", margin: 4 }),
    { mode: 0o600 },
  );
  console.log(message);
  console.log(await QRCode.toString(url, { type: "terminal", small: true }));
  console.log(`QR image: ${join(directory, "connection.svg")}`);
  if (["localhost", "127.0.0.1", "[::1]"].includes(new URL(url).hostname))
    console.log(
      "For a phone, restart with --public-host YOUR_TAILSCALE_IP_OR_NAME so the QR points to this computer.",
    );
}
