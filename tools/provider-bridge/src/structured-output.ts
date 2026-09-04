import { invalidJsonOutput } from "./errors.js";

type Schema = Readonly<Record<string, unknown>>;
type JsonObject = Readonly<Record<string, unknown>>;

export function resolveStructuredOutput(
  provider: string,
  schema: Schema,
  structured: unknown,
  fallbackText: string,
): unknown {
  const root = parseObject(structured) ?? parseObject(fallbackText);
  if (root === undefined) {
    throw invalidJsonOutput(
      provider,
      fallbackText || JSON.stringify(structured) || String(structured),
      new Error("top-level object is missing"),
    );
  }
  return unwrapSingleRequiredObject(root, requiredKeys(schema));
}

function parseObject(value: unknown): JsonObject | undefined {
  if (isObject(value)) return value;
  if (typeof value !== "string" || value.trim().length === 0) return undefined;
  try {
    const parsed = JSON.parse(value) as unknown;
    return isObject(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

function unwrapSingleRequiredObject(root: JsonObject, required: readonly string[]): JsonObject {
  if (required.length === 0 || required.some((key) => Object.hasOwn(root, key))) return root;
  return Object.values(root).find((value): value is JsonObject =>
    isObject(value) && required.every((key) => Object.hasOwn(value, key))) ?? root;
}

function requiredKeys(schema: Schema): readonly string[] {
  return Array.isArray(schema.required)
    ? schema.required.filter((key): key is string => typeof key === "string")
    : [];
}

function isObject(value: unknown): value is JsonObject {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
