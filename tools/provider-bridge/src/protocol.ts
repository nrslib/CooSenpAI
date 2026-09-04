import { isAbsolute } from "node:path";
import type { Provider, ProviderSession } from "./types.js";
import { BridgeError } from "./errors.js";

export const PROTOCOL_VERSION = 2;
// 256 KiB のテキスト添付を二重の JSON escape と envelope 込みで運ぶ。
export const REQUEST_LINE_MAX_BYTES = 2 * 1024 * 1024;
export const RESPONSE_LINE_MAX_BYTES = 2 * 1024 * 1024;
export const DELTA_MAX_BYTES = 256 * 1024;
export const STREAM_MAX_BYTES = 8 * 1024 * 1024;
export const PENDING_REQUEST_LIMIT = 32;

interface RequestBase {
  readonly id: string;
  readonly op: "open" | "resolve" | "send" | "append" | "cancel" | "close";
}

export interface OpenRequest extends RequestBase {
  readonly op: "open";
  readonly provider: Provider;
}

export interface SendRequest extends RequestBase {
  readonly op: "send";
  readonly provider: Provider;
  readonly session: ProviderSession;
  readonly model?: string;
  readonly effort?: string;
  readonly systemPrompt: string;
  readonly message: string;
  readonly images: readonly string[];
  readonly schema?: Record<string, unknown>;
  readonly executable?: string;
  readonly cwd: string;
  readonly toolsDisabled: boolean;
  readonly timeoutMs: number;
}

export interface ResolveRequest extends RequestBase {
  readonly op: "resolve";
  readonly provider: Provider;
  readonly model?: string;
  readonly executable?: string;
  readonly cwd: string;
}

export interface CancelRequest extends RequestBase {
  readonly op: "cancel";
  readonly targetId: string;
}

export interface AppendRequest extends RequestBase {
  readonly op: "append";
  readonly targetId: string;
  readonly message: string;
  readonly images: readonly string[];
}

export interface CloseRequest extends RequestBase {
  readonly op: "close";
}

export type BridgeRequest = OpenRequest | ResolveRequest | SendRequest | AppendRequest | CancelRequest | CloseRequest;

const BASE_KEYS = ["id", "op"] as const;
const OPEN_KEYS = [...BASE_KEYS, "provider"] as const;
const RESOLVE_KEYS = [...BASE_KEYS, "provider", "model", "executable", "cwd"] as const;
const SEND_KEYS = [
  ...BASE_KEYS,
  "provider",
  "session",
  "model",
  "effort",
  "systemPrompt",
  "message",
  "images",
  "schema",
  "executable",
  "cwd",
  "toolsDisabled",
  "timeoutMs",
] as const;
const CANCEL_KEYS = [...BASE_KEYS, "targetId"] as const;
const APPEND_KEYS = [...BASE_KEYS, "targetId", "message", "images"] as const;

function object(value: unknown, name: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new BridgeError("protocol", `${name} must be an object`);
  }
  return value as Record<string, unknown>;
}

function exactKeys(record: Record<string, unknown>, allowed: readonly string[]): void {
  const unknown = Object.keys(record).find((key) => !allowed.includes(key));
  if (unknown !== undefined) throw new BridgeError("protocol", `unknown request field: ${unknown}`);
}

function string(record: Record<string, unknown>, key: string, maxBytes = 1024 * 1024): string {
  const value = record[key];
  if (typeof value !== "string" || value.length === 0 || Buffer.byteLength(value, "utf8") > maxBytes) {
    throw new BridgeError("protocol", `${key} must be a non-empty bounded string`);
  }
  return value;
}

function optionalString(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  if (value === undefined) return undefined;
  if (typeof value !== "string" || value.length === 0 || Buffer.byteLength(value, "utf8") > 4096) {
    throw new BridgeError("protocol", `${key} must be a bounded string`);
  }
  return value;
}

function provider(record: Record<string, unknown>): Provider {
  const value = record.provider;
  if (value !== "codex" && value !== "claude" && value !== "opencode") {
    throw new BridgeError("protocol", "provider is invalid");
  }
  return value;
}

function session(record: Record<string, unknown>): ProviderSession {
  const value = object(record.session, "session");
  exactKeys(value, ["mode", "id"]);
  if (value.mode !== "new" && value.mode !== "resume" && value.mode !== "ephemeral") {
    throw new BridgeError("protocol", "session.mode is invalid");
  }
  const id = value.id;
  if (value.mode === "resume" && (typeof id !== "string" || id.length === 0)) {
    throw new BridgeError("protocol", "resume session ID is required");
  }
  if (id !== undefined && (typeof id !== "string" || id.length === 0 || id.length > 512)) {
    throw new BridgeError("protocol", "session.id is invalid");
  }
  return { mode: value.mode, ...(typeof id === "string" ? { id } : {}) };
}

function parseSend(record: Record<string, unknown>, id: string): SendRequest {
  exactKeys(record, SEND_KEYS);
  const images = record.images;
  if (!Array.isArray(images) || images.some((path) => typeof path !== "string" || !isAbsolute(path))) {
    throw new BridgeError("protocol", "images must contain absolute paths");
  }
  const schema = record.schema;
  if (schema !== undefined && (schema === null || typeof schema !== "object" || Array.isArray(schema))) {
    throw new BridgeError("protocol", "schema must be an object");
  }
  if (schema !== undefined && Buffer.byteLength(JSON.stringify(schema), "utf8") > 512 * 1024) {
    throw new BridgeError("protocol", "schema exceeds the byte limit");
  }
  if (record.toolsDisabled !== true) throw new BridgeError("protocol", "toolsDisabled must be true");
  if (typeof record.timeoutMs !== "number" || !Number.isSafeInteger(record.timeoutMs) || record.timeoutMs <= 0) {
    throw new BridgeError("protocol", "timeoutMs must be a positive integer");
  }
  const cwd = string(record, "cwd", 4096);
  const model = optionalString(record, "model");
  const effort = optionalString(record, "effort");
  const executable = optionalString(record, "executable");
  if (!isAbsolute(cwd)) throw new BridgeError("protocol", "cwd must be absolute");
  return {
    id,
    op: "send",
    provider: provider(record),
    session: session(record),
    ...(model === undefined ? {} : { model }),
    ...(effort === undefined ? {} : { effort }),
    systemPrompt: string(record, "systemPrompt"),
    message: string(record, "message"),
    images: images as string[],
    ...(schema === undefined ? {} : { schema: schema as Record<string, unknown> }),
    ...(executable === undefined ? {} : { executable }),
    cwd,
    toolsDisabled: true,
    timeoutMs: record.timeoutMs,
  };
}

export function parseRequestLine(line: string): BridgeRequest {
  if (Buffer.byteLength(line, "utf8") > REQUEST_LINE_MAX_BYTES) {
    throw new BridgeError("protocol", "request line exceeds the byte limit");
  }
  let raw: unknown;
  try {
    raw = JSON.parse(line) as unknown;
  } catch (error) {
    throw new BridgeError("protocol", "request is not valid JSON", { cause: error });
  }
  const record = object(raw, "request");
  const id = string(record, "id", 128);
  switch (record.op) {
    case "open":
      exactKeys(record, OPEN_KEYS);
      return { id, op: "open", provider: provider(record) };
    case "resolve": {
      exactKeys(record, RESOLVE_KEYS);
      const cwd = string(record, "cwd", 4096);
      if (!isAbsolute(cwd)) throw new BridgeError("protocol", "cwd must be absolute");
      const model = optionalString(record, "model");
      const executable = optionalString(record, "executable");
      return {
        id,
        op: "resolve",
        provider: provider(record),
        ...(model === undefined ? {} : { model }),
        ...(executable === undefined ? {} : { executable }),
        cwd,
      };
    }
    case "send":
      return parseSend(record, id);
    case "append": {
      exactKeys(record, APPEND_KEYS);
      const images = record.images;
      if (!Array.isArray(images) || images.some((path) => typeof path !== "string" || !isAbsolute(path))) {
        throw new BridgeError("protocol", "images must contain absolute paths");
      }
      return {
        id,
        op: "append",
        targetId: string(record, "targetId", 128),
        message: string(record, "message"),
        images: images as string[],
      };
    }
    case "cancel":
      exactKeys(record, CANCEL_KEYS);
      return { id, op: "cancel", targetId: string(record, "targetId", 128) };
    case "close":
      exactKeys(record, BASE_KEYS);
      return { id, op: "close" };
    default:
      throw new BridgeError("protocol", "op is invalid");
  }
}
