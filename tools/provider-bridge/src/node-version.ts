import { BridgeError } from "./errors.js";

export const MINIMUM_NODE_MAJOR = 18;

export function requireSupportedNode(version: string): void {
  const major = Number.parseInt(version.split(".", 1)[0] ?? "", 10);
  if (!Number.isSafeInteger(major) || major < MINIMUM_NODE_MAJOR) {
    throw new BridgeError("unsupported", `Node.js ${MINIMUM_NODE_MAJOR} 以上が必要です`);
  }
}
