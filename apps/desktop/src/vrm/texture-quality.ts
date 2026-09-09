export const VRM_IMAGE_QUALITIES = ["low", "medium", "high"] as const;
export type VrmImageQuality = (typeof VRM_IMAGE_QUALITIES)[number];

export const DEFAULT_VRM_IMAGE_QUALITY: VrmImageQuality = "medium";

export interface VrmImageDimensions {
  readonly width: number;
  readonly height: number;
}

const QUALITY_PROFILES: Readonly<Record<VrmImageQuality, {
  readonly maxDisplayEdge: number;
  readonly maxDisplayPixels: number;
  readonly maxGpuPixels: number;
}>> = {
  // GPU予算は、表示画素予算に完全mipmapの約4/3とsampler差分の余白を加えたアプリ固有値。
  low: { maxDisplayEdge: 1024, maxDisplayPixels: 32 * 1024 * 1024, maxGpuPixels: 48 * 1024 * 1024 },
  medium: { maxDisplayEdge: 2048, maxDisplayPixels: 64 * 1024 * 1024, maxGpuPixels: 96 * 1024 * 1024 },
  high: { maxDisplayEdge: 4096, maxDisplayPixels: 96 * 1024 * 1024, maxGpuPixels: 144 * 1024 * 1024 },
};

export function isVrmImageQuality(value: unknown): value is VrmImageQuality {
  return typeof value === "string" && (VRM_IMAGE_QUALITIES as readonly string[]).includes(value);
}

export function vrmTextureQualityProfile(quality: VrmImageQuality): {
  readonly maxDisplayEdge: number;
  readonly maxDisplayPixels: number;
  readonly maxGpuPixels: number;
} {
  return QUALITY_PROFILES[quality];
}

export function resizedVrmImageDimensions(width: number, height: number, quality: VrmImageQuality): VrmImageDimensions {
  const maxEdge = vrmTextureQualityProfile(quality).maxDisplayEdge;
  const scale = Math.min(1, maxEdge / Math.max(width, height));
  return {
    width: Math.max(1, Math.floor(width * scale)),
    height: Math.max(1, Math.floor(height * scale)),
  };
}

/**
 * 長辺制限後の画像を、選択した画質の表示画素予算内へ一括で収める。
 * 元画像の寸法ではなく、表示用 bitmap の寸法だけを対象にする。
 */
export function fitVrmImageDimensions(
  images: readonly VrmImageDimensions[],
  quality: VrmImageQuality,
): VrmImageDimensions[] {
  const maxPixels = vrmTextureQualityProfile(quality).maxDisplayPixels;
  const edgeLimited = images.map((image) => resizedVrmImageDimensions(image.width, image.height, quality));
  const totalPixels = edgeLimited.reduce((sum, image) => sum + image.width * image.height, 0);
  const scale = totalPixels > maxPixels ? Math.sqrt(maxPixels / totalPixels) : 1;
  return edgeLimited.map((image) => ({
    width: Math.max(1, Math.floor(image.width * scale)),
    height: Math.max(1, Math.floor(image.height * scale)),
  }));
}
