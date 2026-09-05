import type { ConfigPatch, CooSenpaiConfig } from "./types.js";

const UNCHANGED = Symbol("unchanged");

export function changedConfigPatch(
  current: CooSenpaiConfig,
  candidate: ConfigPatch | CooSenpaiConfig,
): ConfigPatch {
  const changed = changedValue(current, candidate);
  return changed === UNCHANGED ? {} : changed as ConfigPatch;
}

function changedValue(current: unknown, candidate: unknown): unknown | typeof UNCHANGED {
  if (isRecord(candidate)) {
    const currentRecord = isRecord(current) ? current : {};
    const entries = Object.entries(candidate).flatMap(([key, value]) => {
      if (key === "revision") return [];
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

export function changedConfigPaths(
  current: CooSenpaiConfig,
  candidate: CooSenpaiConfig,
): readonly string[] {
  return patchPaths(changedConfigPatch(current, candidate));
}

function patchPaths(value: unknown, prefix = ""): string[] {
  if (!isRecord(value)) return prefix === "" ? [] : [prefix];
  const entries = Object.entries(value);
  if (entries.length === 0) return prefix === "" ? [] : [prefix];
  return entries.flatMap(([key, child]) => patchPaths(child, prefix === "" ? key : `${prefix}.${key}`));
}

export function mergeConfigPatch(current: CooSenpaiConfig, patch: ConfigPatch): CooSenpaiConfig {
  return mergeValue(current, patch) as CooSenpaiConfig;
}

function mergeValue(current: unknown, patch: unknown): unknown {
  if (!isRecord(patch)) return patch;
  const currentRecord = isRecord(current) ? current : {};
  const merged: Record<string, unknown> = { ...currentRecord };
  for (const [key, value] of Object.entries(patch)) {
    merged[key] = mergeValue(currentRecord[key], value);
  }
  return merged;
}
