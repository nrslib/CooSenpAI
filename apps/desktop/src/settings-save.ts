import type { ConfigPatch, CooSenpaiConfig } from "./types.js";

const UNCHANGED = Symbol("unchanged");

export function changedConfigPatch(
  current: CooSenpaiConfig,
  candidate: ConfigPatch,
): ConfigPatch {
  const changed = changedValue(current, candidate);
  return changed === UNCHANGED ? {} : changed as ConfigPatch;
}

function changedValue(current: unknown, candidate: unknown): unknown | typeof UNCHANGED {
  if (isRecord(candidate)) {
    const currentRecord = isRecord(current) ? current : {};
    const entries = Object.entries(candidate).flatMap(([key, value]) => {
      const changed = changedValue(currentRecord[key], value);
      return changed === UNCHANGED ? [] : [[key, changed] as const];
    });
    return entries.length === 0 ? UNCHANGED : Object.fromEntries(entries);
  }
  return JSON.stringify(current) === JSON.stringify(candidate) ? UNCHANGED : candidate;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
