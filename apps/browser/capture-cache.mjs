export class CaptureCache {
  generation = 0;
  reply(capture, known) {
    if (capture !== this.capture) {
      if (!Number.isSafeInteger(this.generation + 1)) throw Error("Capture generation exhausted");
      this.capture = capture;
      this.generation++;
    }
    return known === this.generation
      ? { generation: this.generation }
      : { generation: this.generation, capture };
  }
}
