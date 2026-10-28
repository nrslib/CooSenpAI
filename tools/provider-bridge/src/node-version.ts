import { BridgeError } from "./errors.js";

export const MINIMUM_NODE_MAJOR = 18;
export const MINIMUM_NODE_MINOR = 14;
const MINIMUM_NODE_VERSION = `${MINIMUM_NODE_MAJOR}.${MINIMUM_NODE_MINOR}.0`;

export function requireSupportedNode(version: string): void {
  const match = /^(\d+)\.(\d+)\.\d+(?:-[0-9A-Za-z.-]+)?$/.exec(version);
  const major = Number(match?.[1]);
  const minor = Number(match?.[2]);
  if (
    !Number.isSafeInteger(major)
    || !Number.isSafeInteger(minor)
    || major < MINIMUM_NODE_MAJOR
    || (major === MINIMUM_NODE_MAJOR && minor < MINIMUM_NODE_MINOR)
  ) {
    throw new BridgeError("unsupported", `Node.js ${MINIMUM_NODE_VERSION} 以上が必要です`);
  }
}
