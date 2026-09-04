import { existsSync } from "node:fs";
import { resolve } from "node:path";

export function observationFrameDirectory(): string | undefined {
  const productRoot = process.env.COOSENPAI_HOME;
  if (productRoot === undefined) return undefined;
  const directory = resolve(productRoot, "state", "frames");
  return existsSync(directory) ? directory : undefined;
}
