import { validateModelResources } from "./resource-budget.js";
import { vrmError } from "./errors.js";

export const MAX_VRM_BYTES = 50 * 1024 * 1024;

export interface VrmModelFile {
  readonly name: string;
  readonly data: ArrayBuffer;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function validateVrmFile(model: VrmModelFile): { json: Record<string, unknown>; binary: Uint8Array<ArrayBuffer> } {
  if (!model.name.toLowerCase().endsWith(".vrm")) throw vrmError("vrm.errors.fileExtension");
  const data = model.data;
  if (data.byteLength > MAX_VRM_BYTES) throw vrmError("vrm.errors.tooLarge");
  if (data.byteLength < 20) throw vrmError("vrm.errors.corrupt");
  const header = new DataView(data);
  if (header.getUint32(0, true) !== 0x46546c67 || header.getUint32(4, true) !== 2
    || header.getUint32(8, true) !== data.byteLength || header.getUint32(16, true) !== 0x4e4f534a) {
    throw vrmError("vrm.errors.notGlb");
  }
  let offset = 12;
  let json: unknown;
  let binary = new Uint8Array(new ArrayBuffer(0));
  let chunks = 0;
  while (offset < data.byteLength) {
    if (offset + 8 > data.byteLength) throw vrmError("vrm.errors.brokenChunk");
    const length = header.getUint32(offset, true);
    const type = header.getUint32(offset + 4, true);
    if (length % 4 || length > data.byteLength - offset - 8) throw vrmError("vrm.errors.brokenChunk");
    if (chunks === 0 && type === 0x4e4f534a && length > 0 && length <= 4 * 1024 * 1024) {
      json = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(data, offset + 8, length)));
    } else if (chunks === 1 && type === 0x004e4942) {
      binary = new Uint8Array(data, offset + 8, length);
    } else throw vrmError("vrm.errors.duplicateChunk");
    chunks++;
    offset += 8 + length;
  }
  if (!record(json) || !record(json.extensions)
    || (!record(json.extensions.VRM) && !record(json.extensions.VRMC_vrm))) {
    throw vrmError("vrm.errors.unsupportedVersion");
  }
  // 持ち込みモデルに、ネットワークや別のローカルファイルを参照させない。
  const pending: { value: unknown; depth: number }[] = [{ value: json, depth: 0 }];
  const unsupported = new Set(["EXT_mesh_gpu_instancing", "EXT_meshopt_compression", "KHR_draco_mesh_compression", "KHR_texture_basisu", "EXT_texture_webp", "EXT_texture_avif"]);
  let entries = 0;
  while (pending.length > 0) {
    const { value, depth } = pending.pop()!;
    if (++entries > 150_000 || depth > 48) throw vrmError("vrm.errors.tooComplex");
    if (typeof value === "number" && !Number.isFinite(value)) throw vrmError("vrm.errors.invalidNumber");
    if (Array.isArray(value)) { for (const item of value) pending.push({ value: item, depth: depth + 1 }); }
    else if (record(value)) {
      if ("uri" in value) throw vrmError("vrm.errors.externalResource");
      if (Object.keys(value).some((key) => unsupported.has(key))) throw vrmError("vrm.errors.unsupportedExtension");
      for (const item of Object.values(value)) pending.push({ value: item, depth: depth + 1 });
    }
  }
  validateModelResources(json, binary.byteLength);
  return { json, binary };
}
