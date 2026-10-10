/** One command may wait for its actual causal presentation after the exact
 * ledger has received a result. Result and frame can arrive in either order. */
export class CausalWait {
  private command: { sequence: bigint; revision?: bigint } | undefined;
  private presented = 0n;
  get pending(): boolean {
    return this.command !== undefined;
  }
  begin(sequence: bigint): void {
    if (this.command) throw new Error("wait for the preceding command");
    this.command = { sequence };
  }
  result(sequence: bigint, status: string, revision: bigint | null): void {
    if (this.command?.sequence !== sequence) return;
    const outcome = status.toLowerCase();
    if (outcome === "applied") {
      if (revision !== null && revision > 0n) this.command.revision = revision;
      this.release();
    } else if (outcome !== "unknown" && outcome !== "dispatched")
      this.command = undefined;
  }
  present(revision: bigint): void {
    this.presented = revision;
    this.release();
  }
  private release(): void {
    if (
      this.command?.revision !== undefined &&
      this.command.revision <= this.presented
    )
      this.command = undefined;
  }
  clear(): void {
    this.command = undefined;
    this.presented = 0n;
  }
}
