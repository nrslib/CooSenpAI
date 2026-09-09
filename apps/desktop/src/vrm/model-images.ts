import { validateVrmFile, type VrmModelFile } from "./model-file.js";
import { integer, list, object } from "./resource-budget.js";
import { vrmError } from "./errors.js";
import { fitVrmImageDimensions, vrmTextureQualityProfile, type VrmImageQuality } from "./texture-quality.js";

const MAX_SOURCE_EDGE = 8192;
const MAX_SOURCE_PIXELS = 32 * 1024 * 1024;
const IMAGE_DECODE_TIMEOUT_MS = 15_000;

export interface VrmImageSource {
  readonly data: Uint8Array<ArrayBuffer>;
  readonly mimeType: string;
  readonly sourceWidth: number;
  readonly sourceHeight: number;
  readonly width: number;
  readonly height: number;
}

function dimensions(data: Uint8Array<ArrayBuffer>, mimeType: unknown): { width: number; height: number } {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  const invalid = (): never => { throw vrmError("vrm.errors.invalidImage"); };
  let width = 0;
  let height = 0;
  if (mimeType === "image/png") {
    if (data.length < 33 || view.getUint32(0) !== 0x89504e47 || view.getUint32(4) !== 0x0d0a1a0a) invalid();
    let offset = 8;
    let ended = false;
    while (offset + 12 <= data.length) {
      const length = view.getUint32(offset);
      const type = view.getUint32(offset + 4);
      if (length > data.length - offset - 12) invalid();
      if (type === 0x49484452) {
        if (offset !== 8 || length !== 13) invalid();
        width = view.getUint32(offset + 8);
        height = view.getUint32(offset + 12);
      } else if (offset === 8 || type === 0x6163544c) invalid();
      offset += length + 12;
      if (type === 0x49454e44) {
        if (length !== 0 || offset !== data.length) invalid();
        ended = true;
        break;
      }
    }
    if (!ended) invalid();
  } else if (mimeType === "image/jpeg") {
    if (data.length < 4 || view.getUint16(0) !== 0xffd8) invalid();
    let offset = 2;
    let ended = false;
    let scan = false;
    while (offset < data.length) {
      if (data[offset++] !== 0xff) { if (scan) continue; invalid(); }
      while (data[offset] === 0xff) offset++;
      if (offset >= data.length) invalid();
      const marker = data[offset++]!;
      if (scan && (marker === 0 || (marker >= 0xd0 && marker <= 0xd7))) continue;
      if (marker === 0xd9) { ended = offset === data.length; break; }
      scan = false;
      if (offset + 2 > data.length) invalid();
      const length = view.getUint16(offset);
      if (length < 2 || length > data.length - offset) invalid();
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
        if (![0xc0, 0xc2].includes(marker) || width !== 0 || length < 8) invalid();
        height = view.getUint16(offset + 3);
        width = view.getUint16(offset + 5);
      }
      if (marker === 0xda) scan = true;
      // DNL による後からの高さ変更は許可しない。
      if (marker === 0xdc) invalid();
      offset += length;
    }
    if (!ended) invalid();
  } else invalid();
  integer(width, MAX_SOURCE_EDGE);
  integer(height, MAX_SOURCE_EDGE);
  if (!width || !height) invalid();
  if (width * height > MAX_SOURCE_PIXELS) throw vrmError("vrm.errors.imagePixelsExceeded");
  return { width, height };
}

export function inspectVrmImages(file: VrmModelFile, quality: VrmImageQuality): VrmImageSource[] {
  const { json, binary } = validateVrmFile(file);
  const views = list(json.bufferViews).map(object);
  const sources = list(json.images).map((entry) => {
    const image = object(entry);
    const view = views[Number(image.bufferView)]!;
    const offset = Number(view.byteOffset ?? 0);
    const data = binary.subarray(offset, offset + Number(view.byteLength));
    const source = dimensions(data, image.mimeType);
    return {
      data,
      mimeType: String(image.mimeType),
      sourceWidth: source.width,
      sourceHeight: source.height,
      width: source.width,
      height: source.height,
    };
  });
  const displayDimensions = fitVrmImageDimensions(sources, quality);
  const images = sources.map((source, index) => ({ ...source, ...displayDimensions[index] }));
  const pixels = images.reduce((sum, image) => sum + image.width * image.height, 0);
  if (pixels > vrmTextureQualityProfile(quality).maxDisplayPixels) {
    throw vrmError("vrm.errors.imageDisplayPixelsExceeded");
  }
  return images;
}

function closeImage(image: ImageBitmap): void {
  image.close();
}

async function awaitImage(
  operation: Promise<ImageBitmap>,
  signal: AbortSignal,
): Promise<ImageBitmap> {
  let abandoned = false;
  let onAbort: (() => void) | undefined;
  let timeout: ReturnType<typeof setTimeout> | undefined;
  const interrupted = new Promise<never>((_, reject) => {
    onAbort = () => {
      abandoned = true;
      reject(vrmError("vrm.errors.aborted"));
    };
    signal.addEventListener("abort", onAbort, { once: true });
    timeout = setTimeout(() => {
      abandoned = true;
      reject(vrmError("vrm.errors.imageDecodeTimeout"));
    }, IMAGE_DECODE_TIMEOUT_MS);
  });
  const owned = operation.then((image) => {
    if (abandoned) closeImage(image);
    return image;
  });
  if (signal.aborted) onAbort?.();
  try {
    return await Promise.race([owned, interrupted]);
  } finally {
    if (onAbort) signal.removeEventListener("abort", onAbort);
    if (timeout !== undefined) clearTimeout(timeout);
  }
}

function decodeBitmap(blob: Blob, options: ImageBitmapOptions): Promise<ImageBitmap> {
  return Promise.resolve().then(() => createImageBitmap(blob, options));
}

function resizeBitmap(image: ImageBitmap, width: number, height: number): Promise<ImageBitmap> {
  return Promise.resolve().then(() => createImageBitmap(image, {
    resizeWidth: width,
    resizeHeight: height,
    resizeQuality: "high",
    premultiplyAlpha: "none",
    colorSpaceConversion: "none",
  }));
}

export async function decodeVrmImages(file: VrmModelFile, signal: AbortSignal, quality: VrmImageQuality): Promise<ImageBitmap[]> {
  const sources = inspectVrmImages(file, quality);
  const images: ImageBitmap[] = [];
  try {
    for (const source of sources) {
      signal.throwIfAborted();
      const sourceImage = await awaitImage(decodeBitmap(new Blob([source.data], { type: source.mimeType }), {
        premultiplyAlpha: "none",
        colorSpaceConversion: "none",
      }), signal);
      let displayImage = sourceImage;
      try {
        if (sourceImage.width !== source.sourceWidth || sourceImage.height !== source.sourceHeight) {
          throw vrmError("vrm.errors.sizeMismatch");
        }
        if (source.width !== source.sourceWidth || source.height !== source.sourceHeight) {
          displayImage = await awaitImage(resizeBitmap(sourceImage, source.width, source.height), signal);
          if (displayImage.width !== source.width || displayImage.height !== source.height) {
            throw vrmError("vrm.errors.sizeMismatch");
          }
          closeImage(sourceImage);
        }
      } catch (cause) {
        if (displayImage !== sourceImage) closeImage(displayImage);
        closeImage(sourceImage);
        throw cause;
      }
      images.push(displayImage);
      signal.throwIfAborted();
    }
    return images;
  } catch (cause) {
    for (const image of images) image.close();
    throw cause;
  }
}
