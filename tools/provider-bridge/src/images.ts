import { readFile, stat } from "node:fs/promises";
import { extname } from "node:path";
import type { ProviderImageAttachment } from "./types.js";
import { BridgeError } from "./errors.js";

export const IMAGE_ITEM_LIMIT = 8;
export const IMAGE_BYTE_LIMIT = 20 * 1024 * 1024;

export function validateImages(images: readonly ProviderImageAttachment[]): void {
  if (images.length > IMAGE_ITEM_LIMIT) {
    throw new BridgeError("unsupported", `画像は ${IMAGE_ITEM_LIMIT} 件までです`);
  }
}

export async function readImage(image: ProviderImageAttachment): Promise<Buffer> {
  const metadata = await stat(image.path).catch((error: unknown) => {
    throw new BridgeError("invalid-output", "添付画像を読み込めません", { cause: error });
  });
  if (!metadata.isFile() || metadata.size > IMAGE_BYTE_LIMIT) {
    throw new BridgeError("unsupported", "添付画像のサイズが上限を超えています");
  }
  return readFile(image.path);
}

export function imageMime(path: string): "image/jpeg" | "image/png" | "image/gif" | "image/webp" {
  switch (extname(path).toLowerCase()) {
    case ".jpg":
    case ".jpeg":
      return "image/jpeg";
    case ".gif":
      return "image/gif";
    case ".webp":
      return "image/webp";
    default:
      return "image/png";
  }
}
