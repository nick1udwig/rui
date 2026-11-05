import { readFileSync, statSync, writeFileSync, renameSync } from "node:fs";
import { join } from "node:path";
export const ASTEROIDS_CONTROLS = [
  "Left",
  "Right",
  "Fire",
  "Thrust",
  "Hyperspace",
  "Insert coin",
  "Start",
];
export class GameSource {
  constructor(directory, labels = ASTEROIDS_CONTROLS) {
    this.directory = directory;
    this.labels = labels;
    this.mask = 0;
    this.versions = labels.map(() => 1);
    this.generation = 1;
    this.slots = 0;
    this.sequence = 0;
    this.inputRevision = 0;
    this.lastInput = 0;
    this.owner = undefined;
    this.frame = { sequence: 1, lines: [] };
    this.release();
  }
  read() {
    const path = join(this.directory, "frame.json");
    if (statSync(path).size > 768 * 1024)
      throw new Error("MAME frame byte ceiling");
    const frame = JSON.parse(readFileSync(path, "utf8"));
    if (
      !Number.isSafeInteger(frame.sequence) ||
      frame.sequence < 1 ||
      !Array.isArray(frame.lines) ||
      frame.lines.length > 2048
    )
      throw new Error("MAME frame bounds");
    for (const line of frame.lines) {
      if (
        !Array.isArray(line) ||
        line.length !== 7 ||
        line.some((x) => !Number.isFinite(x)) ||
        line.slice(0, 4).some((x) => Math.abs(x) > 4096) ||
        line.slice(4).some((x) => x < 0 || x > 1)
      )
        throw new Error("MAME vector bounds");
    }
    if (frame.sequence !== this.sequence) {
      this.sequence = frame.sequence;
      this.slots = Math.max(this.slots, frame.lines.length);
      this.frame = frame;
      this.generation++;
    }
    return frame;
  }
  capture() {
    this.read();
    return {
      status:
        "MAME vector beams; fixed-width opaque lines; CRT glow and audio excluded",
      nodes: [
        {
          id: 3,
          version: 1,
          role: "root",
          label: "Vector arcade",
          children: this.labels.map((_, i) => 4 + i),
        },
        ...this.labels.map((label, i) => ({
          id: 4 + i,
          version: this.versions[i],
          role: "checkbox",
          label,
          value: Boolean(this.mask & (1 << i)),
          actions: ["SetValue"],
        })),
      ],
      vectors: {
        sequence: this.sequence,
        slots: this.slots,
        lines: this.frame.lines,
      },
    };
  }
  write() {
    this.inputRevision++;
    const path = join(this.directory, "inputs");
    writeFileSync(
      path + ".tmp",
      `${Math.floor(Date.now() / 1000) + 2} ${this.mask} ${this.inputRevision}\n`,
      { mode: 0o600 },
    );
    renameSync(path + ".tmp", path);
  }
  release() {
    for (let i = 0; i < this.labels.length; i++)
      if (this.mask & (1 << i)) this.versions[i]++;
    this.mask = 0;
    this.owner = undefined;
    this.generation++;
    this.write();
  }
  watchdog() {
    if (this.mask && Date.now() - this.lastInput > 350) this.release();
  }
  async dispatch(request) {
    const i = request.target - 4;
    if (
      !Number.isInteger(i) ||
      i < 0 ||
      i >= this.labels.length ||
      request.action !== "SetValue" ||
      typeof request.value !== "boolean" ||
      request.version !== this.versions[i] ||
      !(request.deadlineMs > 0 && request.deadlineMs <= 2000)
    )
      return { status: "rejected" };
    const owner = `${request.epoch}:${request.client}:${request.lease}:${request.generation}`;
    if (this.owner && this.owner !== owner) this.release();
    this.owner = owner;
    const next = request.value ? this.mask | (1 << i) : this.mask & ~(1 << i);
    if (next !== this.mask) {
      this.versions[i]++;
      this.generation++;
    }
    this.mask = next;
    this.lastInput = Date.now();
    this.write();
    const revision = this.inputRevision;
    const deadline = Date.now() + Math.min(request.deadlineMs, 300);
    while (Date.now() < deadline) {
      const frame = this.read();
      if (frame.inputRevision === revision && frame.mask === next)
        return { status: "dispatched" };
      await new Promise((resolve) => setTimeout(resolve, 4));
    }
    this.release();
    return { status: "unknown" };
  }
}
