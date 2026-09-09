import { vrmError } from "./errors.js";
import { LoadingManager } from "three";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { VRMAnimation, VRMAnimationLoaderPlugin } from "@pixiv/three-vrm-animation";

export interface CooMotions {
  readonly idle: VRMAnimation;
  readonly reply: VRMAnimation;
}

export const bundledMotions = {
  idle: { url: "/motions/idle-7.vrma", bytes: 492752, sha256: "a4185076435c7797d169a8e2bd8206696805c46d91d8194ecbe589cd6be24d34" },
  reply: { url: "/motions/idle-talking-6.vrma", bytes: 560376, sha256: "1648d8a03d7e563cad203fa88b80cfd7629e70292cbcc406bc730ac1ffc8bf60" },
} as const;

async function loadMotion(asset: typeof bundledMotions[keyof typeof bundledMotions], signal: AbortSignal): Promise<VRMAnimation> {
  const response = await fetch(asset.url, { signal, redirect: "error" });
  if (!response.ok || !response.body) throw vrmError("vrm.errors.motion.bundledLoad");
  const reader = response.body.getReader();
  const data = new Uint8Array(asset.bytes);
  let offset = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      if (value.byteLength > data.length - offset) throw vrmError("vrm.errors.motion.bundledSize");
      data.set(value, offset);
      offset += value.byteLength;
    }
  } finally { await reader.cancel(); reader.releaseLock(); }
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", data));
  const hash = Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
  // 持ち込みファイルは受け付けず、監査済みの同梱バイナリだけをGLTFパーサーに渡す。
  if (offset !== asset.bytes || hash !== asset.sha256) throw vrmError("vrm.errors.motion.bundledIntegrity");
  signal.throwIfAborted();
  const manager = new LoadingManager();
  manager.setURLModifier(() => { throw vrmError("vrm.errors.motion.bundledExternal"); });
  const loader = new GLTFLoader(manager);
  loader.register(parser => new VRMAnimationLoaderPlugin(parser));
  const gltf = await loader.parseAsync(data.buffer, "");
  signal.throwIfAborted();
  const animations: unknown = gltf.userData.vrmAnimations;
  if (!Array.isArray(animations) || animations.length !== 1 || !(animations[0] instanceof VRMAnimation)) throw vrmError("vrm.errors.motion.bundledFormat");
  return animations[0];
}

export async function loadCooMotions(signal: AbortSignal): Promise<CooMotions> {
  const [idle, reply] = await Promise.all([loadMotion(bundledMotions.idle, signal), loadMotion(bundledMotions.reply, signal)]);
  return { idle, reply };
}
