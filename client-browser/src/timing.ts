/** Local monotonic durations only. Never stores packets, credentials or text. */
export type TimingStage =
  | "receipt"
  | "resourceWait"
  | "wasmExport"
  | "preparation"
  | "composition"
  | "outputConversion"
  | "submission"
  | "presentationOpportunity"
  | "commandResult"
  | "actionToPresentation";
export interface TimingSample {
  stage: TimingStage;
  revision: string;
  durationMs: number;
  count: number;
}
export class Timings {
  private samples: (TimingSample | undefined)[];
  private next = 0;
  private total = 0;
  private commands = new Map<bigint, { started: number; revision?: bigint }>();
  private presentations = new Map<bigint, number>();
  private correlationDropped = 0;
  constructor(readonly capacity = 4096) {
    if (!Number.isInteger(capacity) || capacity < 1 || capacity > 16384)
      throw new Error("timing capacity limit");
    this.samples = new Array(capacity);
  }
  record(
    stage: TimingStage,
    revision: bigint,
    durationMs: number,
    count = 1,
  ): void {
    if (
      !Number.isFinite(durationMs) ||
      durationMs < 0 ||
      !Number.isSafeInteger(count) ||
      count < 0
    )
      throw new Error("invalid timing");
    this.samples[this.next] = {
      stage,
      revision: revision.toString(),
      durationMs,
      count,
    };
    this.next = (this.next + 1) % this.capacity;
    this.total++;
  }
  command(sequence: bigint): void {
    if (this.commands.size >= 128) {
      this.commands.delete(this.commands.keys().next().value!);
      this.correlationDropped++;
    }
    this.commands.set(sequence, { started: performance.now() });
  }
  result(sequence: bigint, revision: bigint | null, status: string): void {
    const command = this.commands.get(sequence);
    if (!command) return;
    this.record(
      "commandResult",
      revision ?? 0n,
      performance.now() - command.started,
    );
    if (status !== "Applied" || revision === null) {
      this.commands.delete(sequence);
      return;
    }
    command.revision = revision;
    const presented = this.presentations.get(revision);
    if (presented !== undefined) {
      this.record(
        "actionToPresentation",
        revision,
        Math.max(0, presented - command.started),
      );
      this.commands.delete(sequence);
    }
  }
  present(revision: bigint): void {
    const now = performance.now();
    if (this.presentations.size >= 128)
      this.presentations.delete(this.presentations.keys().next().value!);
    this.presentations.set(revision, now);
    for (const [sequence, command] of this.commands)
      if (command.revision !== undefined && command.revision <= revision) {
        this.record("actionToPresentation", revision, now - command.started);
        this.commands.delete(sequence);
      }
  }
  disconnect(): void {
    this.correlationDropped += this.commands.size;
    this.commands.clear();
    this.presentations.clear();
  }
  report() {
    const size = Math.min(this.total, this.capacity),
      start = this.total < this.capacity ? 0 : this.next;
    const samples = Array.from({ length: size }, (_, i) => ({
      ...this.samples[(start + i) % this.capacity]!,
    }));
    const stages: Record<
      string,
      { count: number; p50: number; p95: number; p99: number }
    > = {};
    for (const stage of new Set(samples.map((s) => s.stage))) {
      const values = samples
        .filter((s) => s.stage === stage)
        .map((s) => s.durationMs)
        .sort((a, b) => a - b);
      const percentile = (p: number) =>
        values[Math.max(0, Math.ceil(values.length * p) - 1)];
      stages[stage] = {
        count: values.length,
        p50: percentile(0.5),
        p95: percentile(0.95),
        p99: percentile(0.99),
      };
    }
    return {
      capacity: this.capacity,
      correlationDropped: this.correlationDropped,
      dropped: Math.max(0, this.total - this.capacity),
      stages,
      samples,
    };
  }
  clear(): void {
    this.samples.fill(undefined);
    this.next = this.total = this.correlationDropped = 0;
    this.commands.clear();
    this.presentations.clear();
  }
}
