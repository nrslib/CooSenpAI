import { safeProviderError } from "./errors.js";
import { parseRequestLine, type BridgeRequest } from "./protocol.js";

const UNCORRELATED_REQUEST_ID = "protocol";

export interface BridgeInputErrorEvent {
  readonly id: string;
  readonly event: "error";
  readonly kind: string;
  readonly message: string;
  readonly detail?: string;
}

export function acceptBridgeRequestLine(
  line: string,
  accept: (request: BridgeRequest) => void,
): BridgeInputErrorEvent | undefined {
  try {
    accept(parseRequestLine(line));
    return undefined;
  } catch (error) {
    const safe = safeProviderError(error);
    return {
      id: readableRequestId(line) ?? UNCORRELATED_REQUEST_ID,
      event: "error",
      kind: safe.kind,
      message: safe.message,
      ...(safe.detail.length === 0 ? {} : { detail: safe.detail }),
    };
  }
}

function readableRequestId(line: string): string | undefined {
  let value: unknown;
  try {
    value = JSON.parse(line) as unknown;
  } catch {
    return undefined;
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) return undefined;
  const id = (value as Record<string, unknown>).id;
  if (typeof id !== "string" || id.length === 0 || Buffer.byteLength(id, "utf8") > 128) {
    return undefined;
  }
  return id;
}
