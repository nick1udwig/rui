import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { resolve, sep } from "node:path";

export const serveClient = (publicRoot) => createServer(async (req, res) => {
  try {
    if (req.url === "/") {
      res.writeHead(200, { "Content-Type": "text/html" });
      res.end("<!doctype html><title>Owned semantic session</title>");
      return;
    }
    const path = resolve(
      publicRoot,
      "." + new URL(req.url, "http://localhost").pathname,
    );
    if (!path.startsWith(publicRoot + sep) || !(await stat(path)).isFile())
      throw Error();
    res.writeHead(200, {
      "Content-Type": path.endsWith(".wasm")
        ? "application/wasm"
        : "text/javascript",
    });
    res.end(await readFile(path));
  } catch {
    res.writeHead(404);
    res.end();
  }
});
export const wait = async (read, condition = Boolean) => {
  const deadline = Date.now() + 15000;
  while (Date.now() < deadline) {
    try {
      const value = await read();
      if (condition(value)) return value;
    } catch {}
    await new Promise((r) => setTimeout(r, 25));
  }
  throw Error("convergence deadline");
};
