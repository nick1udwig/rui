import { createServer as createSocket } from "node:net";
import { spawn } from "node:child_process";
import { readFileSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createServer } from "vite";
import { GameSource } from "./model.mjs";
const directory = process.env.RUI_STATE;
const native = join(directory, "mame");
mkdirSync(native, { recursive: true, mode: 0o700 });
const game = process.env.RUI_GAME;
if (game !== "asteroid") throw new Error("unsupported game");
const log = (await import("node:fs")).openSync(
  join(directory, "mame.log"),
  "w",
);
const args = [
  game,
  "-rompath",
  process.env.RUI_ROMPATH,
  "-skip_gameinfo",
  "-noplugins",
  "-noreadconfig",
  "-window",
  "-nomaximize",
  "-resolution",
  "640x480",
  "-video",
  "soft",
  "-sound",
  "none",
  "-autoboot_delay",
  "0",
  "-autoboot_script",
  resolve(import.meta.dirname, "mame.lua"),
  "-homepath",
  native,
  "-cfg_directory",
  native,
  "-nvram_directory",
  native,
  "-input_directory",
  native,
  "-snapshot_directory",
  native,
];
if (process.env.RUI_MAME_PLUGINS)
  args.push("-pluginspath", process.env.RUI_MAME_PLUGINS);
const child = spawn(process.env.RUI_MAME, args, {
  env: { ...process.env, RUI_GAME_STATE: native },
  stdio: ["ignore", log, log],
});
let exiting = false;
child.on("error", (error) => {
  console.error(error);
  process.exit(1);
});
child.on("exit", (code) => {
  if (!exiting) {
    console.error(readFileSync(join(directory, "mame.log"), "utf8"));
    process.exit(code || 1);
  }
});
const source = new GameSource(native);
const deadline = Date.now() + 20000;
while (!existsSync(join(native, "frame.json"))) {
  if (Date.now() > deadline)
    throw new Error("MAME vector capture did not start; see mame.log");
  await new Promise((resolve) => setTimeout(resolve, 30));
}
source.capture();
let requests = 0;
const server = createSocket((socket) => {
  let buffer = Buffer.alloc(0),
    chain = Promise.resolve();
  socket.on("data", (bytes) => {
    buffer = Buffer.concat([buffer, bytes]);
    if (buffer.length > 131072) return socket.destroy();
    while (buffer.length >= 4) {
      const length = buffer.readUInt32BE();
      if (length > 65536) return socket.destroy();
      if (buffer.length < length + 4) break;
      const body = buffer.subarray(4, length + 4);
      buffer = buffer.subarray(length + 4);
      if (++requests > 16) return socket.destroy();
      chain = chain.then(async () => {
        let response;
        try {
          const request = JSON.parse(body);
          if (request.op === "capture") response = source.capture();
          else if (request.op === "captureIfChanged") {
            source.read();
            response = {
              generation: source.generation,
              ...(request.generation === source.generation
                ? {}
                : { capture: source.capture() }),
            };
          } else if (request.op === "dispatch")
            response = await source.dispatch(request);
          else if (request.op === "releaseInputs") {
            source.release();
            response = { ok: true };
          } else throw new Error("unsupported operation");
          const payload = Buffer.from(JSON.stringify(response));
          const header = Buffer.alloc(4);
          header.writeUInt32BE(payload.length);
          socket.write(Buffer.concat([header, payload]));
        } catch (error) {
          console.error(error);
          socket.destroy();
        } finally {
          requests--;
        }
      });
    }
  });
  socket.on("error", () => source.release());
  socket.on("close", () => source.release());
});
server.listen(join(directory, "worker.sock"));
const timer = setInterval(() => source.watchdog(), 25);
while (!existsSync(join(directory, "backend.json")))
  await new Promise((resolve) => setTimeout(resolve, 20));
const backend = JSON.parse(
  readFileSync(join(directory, "backend.json"), "utf8"),
);
process.env.RUI_DEMO_IDENTITY = directory;
process.env.RUI_HOST_URL = `wss://127.0.0.1:${backend.port}`;
const web = await createServer({
  root: import.meta.dirname,
  configFile: resolve(import.meta.dirname, "vite.config.ts"),
});
await web.listen();
console.log(
  `Vector arcade: ${process.env.RUI_ORIGIN}\nCredential: ${join(directory, "client.credential")}`,
);
async function stop() {
  if (exiting) return;
  exiting = true;
  source.release();
  clearInterval(timer);
  child.kill("SIGTERM");
  server.close();
  await web.close();
  setTimeout(() => {
    child.kill("SIGKILL");
    process.exit(0);
  }, 500).unref();
}
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
