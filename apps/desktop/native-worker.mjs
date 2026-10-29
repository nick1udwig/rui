import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const script = fileURLToPath(new URL("./native.py", import.meta.url));
export class NativeWorker {
  constructor(changed, command = ["/usr/bin/python3", script, "--worker"]) {
    this.changed = changed;
    this.command = command;
    this.queue = [];
    this.closed = false;
  }
  request(request) {
    if (this.closed || this.queue.length >= 32)
      return Promise.reject(Error("Accessibility worker unavailable or busy"));
    const body = Buffer.from(JSON.stringify(request) + "\n");
    if (body.length > 65536)
      return Promise.reject(Error("Native request limit"));
    return new Promise((resolve, reject) => {
      this.queue.push({ body, resolve, reject });
      this.drain();
    });
  }
  start() {
    const child = spawn(this.command[0], this.command.slice(1), {
      stdio: ["pipe", "pipe", "ignore"],
    });
    this.child = child;
    let buffer = Buffer.alloc(0);
    const fail = () => {
      if (this.child === child)
        this.fail(Error("Accessibility worker unavailable or timed out"));
    };
    child.on("error", fail);
    child.on("exit", fail);
    child.stdin.on("error", fail);
    child.stdout.on("data", (bytes) => {
      if (this.child !== child) return;
      if (buffer.length + bytes.length > 1024 * 1024) return fail();
      buffer = Buffer.concat([buffer, bytes]);
      for (;;) {
        const end = buffer.indexOf(10);
        if (end < 0) break;
        let result;
        try {
          result = JSON.parse(buffer.subarray(0, end));
        } catch {
          return fail();
        }
        buffer = buffer.subarray(end + 1);
        if (!result || typeof result !== "object" || Array.isArray(result))
          return fail();
        if (result.dirty === true) {
          this.changed();
          continue;
        }
        if (!this.pending) return fail();
        clearTimeout(this.timer);
        const pending = this.pending;
        this.pending = undefined;
        if (result.error)
          pending.reject(
            Error(
              result.detail ? `${result.error} ${result.detail}` : result.error,
            ),
          );
        else pending.resolve(result);
        this.drain();
      }
    });
  }
  drain() {
    if (this.pending || !this.queue.length || this.closed) return;
    if (!this.child) this.start();
    this.pending = this.queue.shift();
    this.timer = setTimeout(
      () => this.fail(Error("Accessibility worker timed out")),
      2400,
    );
    this.child.stdin.write(this.pending.body);
  }
  fail(error) {
    const child = this.child;
    this.child = undefined;
    clearTimeout(this.timer);
    child?.kill("SIGKILL");
    this.pending?.reject(error);
    this.pending = undefined;
    for (const pending of this.queue.splice(0)) pending.reject(error);
  }
  close() {
    this.closed = true;
    this.fail(Error("Accessibility worker closed"));
  }
}
