import { existsSync } from "node:fs";
import { resolve } from "node:path";

export function observationDirectories(productRoot = process.env.COOSENPAI_HOME): string[] {
  if (productRoot === undefined) return [];
  return ["frames", "transcripts"]
    .map((name) => resolve(productRoot, "state", name))
    .filter((directory) => existsSync(directory));
}
