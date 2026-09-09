import { Material, Object3D, RGBAFormat, ShaderMaterial, Source, Texture, UnsignedByteType } from "three";
import { vrmError } from "./errors.js";
import { vrmTextureQualityProfile, type VrmImageQuality } from "./texture-quality.js";

export function inspectVrmTextures(scene: Object3D, quality: VrmImageQuality): { pixels: number; allocations: number } {
  const maxPixels = vrmTextureQualityProfile(quality).maxGpuPixels;
  const textures = new Set<Texture>();
  scene.traverse((node) => {
    if (!("material" in node)) return;
    const materials: unknown[] = Array.isArray(node.material) ? node.material : [node.material];
    for (const material of materials) {
      if (!(material instanceof Material)) continue;
      for (const value of Object.values(material)) if (value instanceof Texture) textures.add(value);
      if (material instanceof ShaderMaterial) {
        for (const uniform of Object.values(material.uniforms)) if (uniform.value instanceof Texture) textures.add(uniform.value);
      }
    }
  });
  const sources = new Map<Source, Set<string>>();
  let pixels = 0;
  let allocations = 0;
  for (const texture of textures) {
    if (texture.format !== RGBAFormat || texture.type !== UnsignedByteType || texture.internalFormat !== null || texture.mipmaps.length) {
      throw vrmError("vrm.errors.textureUnsupportedFormat");
    }
    // three r180 WebGLTextures の共有条件。UV変換やchannelはGPU画像を複製しない。
    const wrapR = (texture as Texture & { readonly wrapR?: number }).wrapR || 0;
    const key = JSON.stringify([
      texture.wrapS, texture.wrapT, wrapR, texture.magFilter, texture.minFilter, texture.anisotropy,
      texture.internalFormat, texture.format, texture.type, texture.generateMipmaps,
      texture.premultiplyAlpha, texture.flipY, texture.unpackAlignment, texture.colorSpace,
    ]);
    let keys = sources.get(texture.source);
    if (keys?.has(key)) continue;
    if (!keys) { keys = new Set(); sources.set(texture.source, keys); }
    keys.add(key);
    let width: number = texture.image?.width;
    let height: number = texture.image?.height;
    if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width < 1 || height < 1 || width > 4096 || height > 4096) {
      throw vrmError("vrm.errors.textureInvalidSize");
    }
    allocations++;
    pixels += width * height;
    if (texture.generateMipmaps) {
      while (width > 1 || height > 1) {
        width = Math.max(1, Math.floor(width / 2));
        height = Math.max(1, Math.floor(height / 2));
        pixels += width * height;
      }
    }
    if (pixels > maxPixels) throw vrmError("vrm.errors.textureBudgetExceeded");
  }
  return { pixels, allocations };
}
