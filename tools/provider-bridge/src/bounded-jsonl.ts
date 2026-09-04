export interface JsonlChunkResult {
  readonly lines: readonly string[];
  readonly oversizedLines: number;
}

export class BoundedJsonlReader {
  private pending = Buffer.alloc(0);
  private dropping = false;

  constructor(private readonly limit: number) {}

  push(chunk: Buffer): JsonlChunkResult {
    const lines: string[] = [];
    let oversizedLines = 0;
    let offset = 0;
    while (offset < chunk.length) {
      const newline = chunk.indexOf(0x0a, offset);
      const end = newline === -1 ? chunk.length : newline;
      const piece = chunk.subarray(offset, end);
      if (!this.dropping) {
        if (this.pending.length + piece.length > this.limit) {
          this.pending = Buffer.alloc(0);
          this.dropping = true;
          oversizedLines += 1;
        } else if (piece.length > 0) {
          this.pending = Buffer.concat([this.pending, piece], this.pending.length + piece.length);
        }
      }
      if (newline === -1) break;
      if (this.dropping) {
        this.dropping = false;
      } else {
        const line = this.pending.at(-1) === 0x0d
          ? this.pending.subarray(0, this.pending.length - 1)
          : this.pending;
        lines.push(line.toString("utf8"));
        this.pending = Buffer.alloc(0);
      }
      offset = newline + 1;
    }
    return { lines, oversizedLines };
  }
}
