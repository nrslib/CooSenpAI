import { Box3, BufferGeometry, ClampToEdgeWrapping, DirectionalLight, HemisphereLight, LinearFilter, LinearMipmapLinearFilter, LinearMipmapNearestFilter, Line, LoadingManager, Material, Mesh, MirroredRepeatWrapping, NearestFilter, NearestMipmapLinearFilter, NearestMipmapNearestFilter, Object3D, PerspectiveCamera, Points, RepeatWrapping, Scene, ShaderMaterial, Skeleton, SkinnedMesh, Texture, Vector3, WebGLRenderer, type MagnificationTextureFilter, type MinificationTextureFilter, type Wrapping } from "three";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { VRM, VRMLoaderPlugin, VRMUtils } from "@pixiv/three-vrm";
import { validateVrmFile, type VrmModelFile } from "./model-file.js";
import { decodeVrmImages } from "./model-images.js";
import { Source } from "three";
import { inspectVrmTextures } from "./texture-budget.js";
import type { CompanionEmotions } from "../types.js";
import { applyVrmEmotions, neutralEmotions } from "./emotions.js";
import { Quaternion } from "three";
import { createCooMotionPlayer } from "./motion-player.js";
import type { AssignedMotions, MotionSlot } from "./motion-settings.js";
import { vrmError } from "./errors.js";
import type { VrmImageQuality } from "./texture-quality.js";

const modelResources = new WeakMap<VRM, () => void>();
const magFilters: Record<number, MagnificationTextureFilter> = { 9728: NearestFilter, 9729: LinearFilter };
const minFilters: Record<number, MinificationTextureFilter> = { ...magFilters, 9984: NearestMipmapNearestFilter, 9985: LinearMipmapNearestFilter, 9986: NearestMipmapLinearFilter, 9987: LinearMipmapLinearFilter };
const wrappings: Record<number, Wrapping> = { 33071: ClampToEdgeWrapping, 33648: MirroredRepeatWrapping, 10497: RepeatWrapping };

class ModelResources {
  readonly pending = new Set<Promise<unknown>>();
  readonly objects = new Set<Object3D>();
  readonly materials = new Set<Material>();
  readonly textures = new Set<Texture>();
  constructor(readonly images: ImageBitmap[]) {}

  track(value: unknown): void {
    if (value instanceof Object3D) this.objects.add(value);
    if (value instanceof Material) this.materials.add(value);
    if (value instanceof Texture) this.textures.add(value);
  }

  dispose(): void {
    const geometries = new Set<BufferGeometry>();
    const skeletons = new Set<Skeleton>();
    for (const root of this.objects) root.traverse((object) => {
      if (object instanceof Mesh || object instanceof Line || object instanceof Points) {
        geometries.add(object.geometry);
        for (const material of Array.isArray(object.material) ? object.material : [object.material]) this.materials.add(material);
      }
      if (object instanceof SkinnedMesh) skeletons.add(object.skeleton);
    });
    for (const material of this.materials) {
      for (const value of Object.values(material)) if (value instanceof Texture) this.textures.add(value);
      if (material instanceof ShaderMaterial) {
        for (const uniform of Object.values(material.uniforms)) if (uniform.value instanceof Texture) this.textures.add(uniform.value);
      }
      material.dispose();
    }
    for (const geometry of geometries) geometry.dispose();
    for (const skeleton of skeletons) skeleton.dispose();
    for (const texture of this.textures) texture.dispose();
    for (const image of this.images) image.close();
    this.objects.clear();
    this.materials.clear();
    this.textures.clear();
    this.images.length = 0;
  }
}

export function disposeVrmModel(vrm: VRM): void {
  modelResources.get(vrm)?.();
  modelResources.delete(vrm);
}

export async function loadVrmModel(file: VrmModelFile, signal: AbortSignal, quality: VrmImageQuality): Promise<VRM> {
  signal.throwIfAborted();
  validateVrmFile(file);
  const resources = new ModelResources(await decodeVrmImages(file, signal, quality));
  const imageSources = resources.images.map((image) => new Source(image));
  const manager = new LoadingManager();
  manager.setURLModifier(() => {
    throw vrmError("vrm.errors.externalResourceLoad");
  });
  const loader = new GLTFLoader(manager);
  loader.register((parser) => {
    const getDependency = parser.getDependency.bind(parser);
    // パーサー単位で追跡し、途中失敗しても並行して生成中の資源を回収する。
    parser.getDependency = (type, index) => {
      const promise: Promise<unknown> = getDependency(type, index);
      resources.pending.add(promise);
      void promise.then((value) => {
        resources.track(value);
        resources.pending.delete(promise);
      }, () => { resources.pending.delete(promise); });
      return promise;
    };
    return {
      name: "CooSenpAIEmbeddedTextures",
      loadTexture: async (index) => {
        signal.throwIfAborted();
        const definition = parser.json.textures[index] as { source: number; sampler?: number; name?: string };
        const sampler = (definition.sampler === undefined ? {} : parser.json.samplers[definition.sampler]) as { magFilter?: number; minFilter?: number; wrapS?: number; wrapT?: number };
        const image = resources.images[definition.source];
        if (!image) throw vrmError("vrm.errors.invalidImageReference");
        const texture = new Texture(image);
        texture.source = imageSources[definition.source]!;
        resources.textures.add(texture);
        texture.flipY = false;
        texture.name = definition.name ?? "";
        const magFilter = magFilters[sampler.magFilter ?? 9729];
        const minFilter = minFilters[sampler.minFilter ?? 9987];
        const wrapS = wrappings[sampler.wrapS ?? 10497];
        const wrapT = wrappings[sampler.wrapT ?? 10497];
        if (typeof magFilter !== "number" || typeof minFilter !== "number" || typeof wrapS !== "number" || typeof wrapT !== "number") throw vrmError("vrm.errors.invalidTextureSettings");
        texture.magFilter = magFilter;
        texture.minFilter = minFilter;
        texture.wrapS = wrapS;
        texture.wrapT = wrapT;
        texture.generateMipmaps = texture.minFilter !== NearestFilter && texture.minFilter !== LinearFilter;
        texture.needsUpdate = true;
        parser.associations.set(texture, { textures: index });
        return texture;
      },
      afterRoot: async (gltf) => { for (const scene of gltf.scenes) resources.track(scene); },
    };
  });
  loader.register((parser) => new VRMLoaderPlugin(parser));
  try {
    signal.throwIfAborted();
    const gltf = await loader.parseAsync(file.data, "");
    signal.throwIfAborted();
    const vrm: unknown = gltf.userData.vrm;
    if (!(vrm instanceof VRM)) throw vrmError("vrm.errors.loadFailed");
    VRMUtils.rotateVRM0(vrm);
    // VRM 0.x と 1.0 ではモデルの正面が異なるため、実際の腕の方向から下げる向きを決める。
    for (const side of ["left", "right"] as const) {
      const upper = vrm.humanoid.getNormalizedBoneNode(`${side}UpperArm`);
      const lower = vrm.humanoid.getNormalizedBoneNode(`${side}LowerArm`);
      if (upper && lower) upper.rotation.z = -Math.sign(lower.position.x) * 1.2;
    }
    vrm.update(0);
    const bounds = new Box3().setFromObject(vrm.scene);
    if (![...bounds.min.toArray(), ...bounds.max.toArray()].every(Number.isFinite) || bounds.max.y <= bounds.min.y) throw vrmError("vrm.errors.invalidMesh");
    inspectVrmTextures(vrm.scene, quality);
    modelResources.set(vrm, () => resources.dispose());
    return vrm;
  } catch (cause) {
    while (resources.pending.size > 0) await Promise.allSettled([...resources.pending]);
    resources.dispose();
    throw cause;
  }
}

export interface VrmSceneHandle {
  dispose(): void;
  update(emotions: CompanionEmotions, thinking: boolean): void;
  reply(): void;
  preview(slot: MotionSlot): void;
  cancelReply(): void;
}

export function mountVrmScene(host: HTMLElement, vrm: VRM, initialEmotions: CompanionEmotions, initialThinking: boolean, onError?: (cause: unknown) => void, motions?: AssignedMotions): VrmSceneHandle {
  let emotions = initialEmotions;
  let thinking = initialThinking;
  const motion = motions === undefined ? undefined : createCooMotionPlayer(vrm, motions);
  motion?.setThinking(initialThinking);
  const animatedHead = new Quaternion();
  const head = vrm.humanoid.getNormalizedBoneNode("head");
  const pose = (delta: number, moving: boolean): void => {
    if (motion && head) head.quaternion.copy(animatedHead);
    motion?.apply(delta, moving);
    if (motion && head) animatedHead.copy(head.quaternion);
    applyVrmEmotions(vrm, emotions, thinking, elapsed, moving);
    if (motion && head) head.quaternion.premultiply(animatedHead);
    vrm.update(delta);
  };
  const box = new Box3().setFromObject(vrm.scene);
  const size = box.getSize(new Vector3());
  const center = box.getCenter(new Vector3());
  const renderer = new WebGLRenderer({ alpha: true, antialias: true });
  renderer.debug.onShaderError = () => { throw vrmError("vrm.errors.shaderFailed"); };
  const scene = new Scene();
  scene.add(vrm.scene, new HemisphereLight(0xffffff, 0x888899, 2));
  const light = new DirectionalLight(0xffffff, 2);
  light.position.set(1, 2, 3);
  scene.add(light);
  const camera = new PerspectiveCamera(30, 1, 0.01, 1000);
  let frame = 0;
  let lastTime = 0;
  let elapsed = 0;
  let disposed = false;
  const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const render = (): void => {
    renderer.render(scene, camera);
    if (renderer.getContext().isContextLost()) throw vrmError("vrm.errors.contextLost");
    if (renderer.getContext().getError() !== renderer.getContext().NO_ERROR) throw vrmError("vrm.errors.gpuFailed");
  };
  const resize = (): void => {
    const width = Math.max(host.clientWidth, 1);
    const height = Math.max(host.clientHeight, 1);
    renderer.setSize(width, height);
    camera.aspect = width / height;
    const distance = Math.max(size.y, size.x / camera.aspect) / (2 * Math.tan(Math.PI / 12)) * 1.12;
    camera.position.set(center.x, center.y, center.z + distance);
    camera.lookAt(center);
    camera.updateProjectionMatrix();
    render();
  };
  const animate = (time: number): void => {
    if (disposed) return;
    try {
      const delta = lastTime === 0 ? 0 : Math.min((time - lastTime) / 1000, 0.05);
      lastTime = time;
      elapsed += delta;
      pose(delta, true);
      render();
      frame = requestAnimationFrame(animate);
    } catch (cause) { fail(cause); }
  };
  const syncAnimation = (): void => {
    if (disposed) return;
    try {
      cancelAnimationFrame(frame);
      lastTime = 0;
      if (reducedMotion.matches) {
        pose(0, false);
        render();
      }
      if (!document.hidden && !reducedMotion.matches) frame = requestAnimationFrame(animate);
    } catch (cause) { fail(cause); }
  };
  const observer = new ResizeObserver(() => {
    if (disposed) return;
    try { resize(); } catch (cause) { fail(cause); }
  });
  const dispose = (): void => {
    if (disposed) return;
    disposed = true;
    cancelAnimationFrame(frame);
    observer.disconnect();
    document.removeEventListener("visibilitychange", syncAnimation);
    reducedMotion.removeEventListener("change", syncAnimation);
    renderer.domElement.removeEventListener("webglcontextlost", contextLost);
    scene.remove(vrm.scene);
    motion?.dispose();
    // renderer.dispose だけでは geometry のモーフ用 CPU 配列が残る。画像は閉じず、再描画に備える。
    VRMUtils.deepDispose(vrm.scene);
    renderer.dispose();
    renderer.forceContextLoss();
    renderer.domElement.remove();
  };
  const fail = (cause: unknown): void => {
    dispose();
    onError?.(cause);
  };
  const contextLost = (): void => { fail(vrmError("vrm.errors.contextLostSelect")); };
  try {
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    renderer.setClearColor(0x000000, 0);
    host.append(renderer.domElement);
    observer.observe(host);
    document.addEventListener("visibilitychange", syncAnimation);
    reducedMotion.addEventListener("change", syncAnimation);
    renderer.domElement.addEventListener("webglcontextlost", contextLost);
    pose(0, false);
    resize();
    syncAnimation();
    if (disposed) throw vrmError("vrm.errors.renderStartFailed");
    return { dispose, reply: () => { if (!disposed && !document.hidden && !reducedMotion.matches) motion?.reply(emotions); },
      preview: (slot) => { if (!disposed && !document.hidden && !reducedMotion.matches) motion?.preview(slot); },
      cancelReply: () => motion?.cancelReply(), update: (nextEmotions, nextThinking) => {
      emotions = nextEmotions;
      thinking = nextThinking;
      motion?.setThinking(nextThinking);
      if (disposed) return;
      if (document.hidden || reducedMotion.matches) {
        try {
          pose(0, false);
          render();
        } catch (cause) { fail(cause); }
      }
    } };
  } catch (cause) {
    dispose();
    throw cause;
  }
}

export function verifyVrmRender(vrm: VRM): void {
  const host = document.createElement("div");
  const scene = mountVrmScene(host, vrm, neutralEmotions(), false);
  scene.dispose();
}
