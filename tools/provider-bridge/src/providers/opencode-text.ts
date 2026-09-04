interface StreamPart {
  readonly id?: unknown;
  readonly type?: unknown;
}

interface StreamEvent {
  readonly type: string;
  readonly properties: object;
}

export class OpenCodeTextProjector {
  private readonly partTypes = new Map<string, string>();
  private readonly pending = new Map<string, string[]>();

  push(event: StreamEvent): string[] {
    const properties = event.properties as {
      readonly part?: StreamPart;
      readonly partID?: unknown;
      readonly field?: unknown;
      readonly delta?: unknown;
    };
    if (event.type === "message.part.updated") {
      const part = properties.part;
      if (typeof part?.id !== "string" || typeof part.type !== "string") return [];
      this.partTypes.set(part.id, part.type);
      const pending = this.pending.get(part.id) ?? [];
      this.pending.delete(part.id);
      return part.type === "text" ? pending : [];
    }
    if (
      event.type !== "message.part.delta"
      || properties.field !== "text"
      || typeof properties.delta !== "string"
    ) return [];
    const partId = properties.partID;
    if (typeof partId !== "string") return [properties.delta];
    const partType = this.partTypes.get(partId);
    if (partType === "text") return [properties.delta];
    if (partType === "reasoning") return [];
    const pending = this.pending.get(partId) ?? [];
    pending.push(properties.delta);
    this.pending.set(partId, pending);
    return [];
  }
}
