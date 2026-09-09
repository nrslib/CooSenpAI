import { InterpolateDiscrete, Quaternion, Vector3, type KeyframeTrack } from "three";
import { VRMHumanBoneList, type VRM } from "@pixiv/three-vrm";
import { createVRMAnimationHumanoidTracks, type VRMAnimation } from "@pixiv/three-vrm-animation";
import type { CompanionEmotions } from "../types.js";
import type { AssignedMotions, MotionSlot } from "./motion-settings.js";

const TRANSITION_SECONDS = 0.6;
const emotionSlots = ["joy", "embarrassment", "concern", "surprise", "curiosity", "frustration"] as const;
const ease = (value: number): number => { const t = Math.max(0, Math.min(1, value)); return t * t * (3 - 2 * t); };
const interpolant = (track: KeyframeTrack) => track.getInterpolation() === InterpolateDiscrete
  ? track.InterpolantFactoryMethodDiscrete() : track.InterpolantFactoryMethodLinear();

function bodySampler(animation: VRMAnimation, vrm: VRM) {
  // 表情・口・視線はCoo側が管理し、素材からは身体のトラックだけを取り込む。
  const tracks = createVRMAnimationHumanoidTracks(animation, vrm.humanoid, vrm.meta.metaVersion);
  const rotations = [...tracks.rotation].map(([name, track]) => {
    track.setInterpolation(animation.humanoidTracks.rotation.get(name)!.getInterpolation());
    return { bone: vrm.humanoid.getNormalizedBoneNode(name)!, sample: interpolant(track) };
  });
  const translations = [...tracks.translation].map(([name, track]) => ({ bone: vrm.humanoid.getNormalizedBoneNode(name)!, sample: interpolant(track) }));
  const rotation = new Quaternion();
  const position = new Vector3();
  return {
    duration: animation.duration,
    apply(time: number, weight: number): void {
      for (const { bone, sample } of rotations) bone.quaternion.slerp(rotation.fromArray(sample.evaluate(time)), weight);
      for (const { bone, sample } of translations) bone.position.lerp(position.fromArray(sample.evaluate(time)), weight);
    },
  };
}

export function createCooMotionPlayer(vrm: VRM, motions: AssignedMotions) {
  const idle = bodySampler(motions.idle, vrm);
  const samplers = new Map<MotionSlot, ReturnType<typeof bodySampler>>([["idle", idle]]);
  for (const [slot, animation] of Object.entries(motions)) {
    if (slot !== "idle" && animation !== undefined) samplers.set(slot as MotionSlot, bodySampler(animation, vrm));
  }
  const rest = VRMHumanBoneList.flatMap(name => {
    const bone = vrm.humanoid.getNormalizedBoneNode(name);
    return bone === null ? [] : [{ bone, rotation: bone.quaternion.clone(), position: bone.position.clone() }];
  });
  const restore = (): void => { for (const { bone, rotation, position } of rest) { bone.quaternion.copy(rotation); bone.position.copy(position); } };
  let elapsed = 0;
  let thinking = false;
  let thinkingWeight = 0;
  let active: { sampler: ReturnType<typeof bodySampler>; elapsed: number } | undefined;
  let pending: MotionSlot | undefined;
  let disposed = false;
  return {
    reply(emotions?: CompanionEmotions): void {
      if (disposed) return;
      const selected = emotions === undefined ? undefined : emotionSlots
        .filter(slot => emotions[slot] >= 50 && samplers.has(slot))
        .sort((a, b) => emotions[b] - emotions[a])[0];
      pending = selected ?? (samplers.has("reply") ? "reply" : undefined);
    },
    preview(slot: MotionSlot): void { if (!disposed && samplers.has(slot)) pending = slot; },
    setThinking(value: boolean): void { thinking = value; },
    cancelReply(): void { pending = undefined; active = undefined; },
    apply(delta: number, moving: boolean): void {
      if (disposed) return;
      if (!moving) {
        pending = undefined; active = undefined; thinkingWeight = 0;
        restore(); idle.apply(0, 1);
        return;
      }
      elapsed += delta;
      if (active === undefined && pending !== undefined) {
        active = { sampler: samplers.get(pending)!, elapsed: 0 };
        pending = undefined;
      }
      const target = thinking && samplers.has("thinking") ? 1 : 0;
      thinkingWeight = target > thinkingWeight ? Math.min(target, thinkingWeight + delta / TRANSITION_SECONDS) : Math.max(target, thinkingWeight - delta / TRANSITION_SECONDS);
      restore();
      idle.apply(elapsed % idle.duration, 1);
      const thinkingSampler = samplers.get("thinking");
      if (thinkingSampler && thinkingWeight > 0) thinkingSampler.apply(elapsed % thinkingSampler.duration, ease(thinkingWeight));
      if (active) {
        active.elapsed += delta;
        const duration = active.sampler.duration;
        const weight = ease(active.elapsed / TRANSITION_SECONDS) * (1 - ease((active.elapsed - duration) / TRANSITION_SECONDS));
        active.sampler.apply(Math.min(active.elapsed, duration), weight);
        if (active.elapsed >= duration + TRANSITION_SECONDS) active = undefined;
      }
    },
    dispose(): void { if (!disposed) { disposed = true; pending = undefined; active = undefined; restore(); } },
  };
}
