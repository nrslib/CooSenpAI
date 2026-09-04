export class DeltaProjector {
  private readonly messageEnvelope: boolean;
  private raw = "";
  private emitted = "";

  constructor(schema: Record<string, unknown> | undefined) {
    const properties = schema?.properties;
    this.messageEnvelope = properties !== null
      && typeof properties === "object"
      && !Array.isArray(properties)
      && Object.prototype.hasOwnProperty.call(properties, "message");
  }

  push(delta: string): string {
    if (!this.messageEnvelope) return delta;
    this.raw += delta;
    const prefix = decodedMessagePrefix(this.raw);
    if (prefix.length <= this.emitted.length) return "";
    const next = prefix.slice(this.emitted.length);
    this.emitted = prefix;
    return next;
  }

  reset(): void {
    this.raw = "";
    this.emitted = "";
  }
}

function decodedMessagePrefix(json: string): string {
  const match = /"message"\s*:\s*"((?:\\.|[^"\\])*)/u.exec(json);
  if (match?.[1] === undefined) return "";
  let encoded = match[1];
  while (encoded.length > 0) {
    try {
      return JSON.parse(`"${encoded}"`) as string;
    } catch {
      encoded = encoded.slice(0, -1);
    }
  }
  return "";
}
