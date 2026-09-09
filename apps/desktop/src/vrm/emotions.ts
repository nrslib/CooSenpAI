import type { VRM } from "@pixiv/three-vrm";
import type { CompanionEmotions } from "../types.js";

export function neutralEmotions(): CompanionEmotions {
  return { joy: 0, embarrassment: 0, concern: 0, surprise: 0, curiosity: 0, frustration: 0 };
}

export function applyVrmEmotions(vrm: VRM, emotions: CompanionEmotions, thinking: boolean, elapsed: number, moving: boolean): void {
  const expressions = { happy: emotions.joy / 100, sad: emotions.concern / 100, angry: emotions.frustration / 100, surprised: emotions.surprise / 100 };
  const total = Math.max(1, Object.values(expressions).reduce((sum, value) => sum + value, 0));
  const hasBlink = Boolean(vrm.expressionManager?.getExpression("blink"));
  const blink = moving && hasBlink ? Math.max(0, 1 - Math.abs((elapsed % 4) - 3.8) / 0.1) : 0;
  for (const [name, weight] of Object.entries(expressions)) {
    if (vrm.expressionManager?.getExpression(name)) vrm.expressionManager.setValue(name, weight / total * 0.8 * (1 - blink));
  }
  if (hasBlink) vrm.expressionManager?.setValue("blink", blink);
  const head = vrm.humanoid.getNormalizedBoneNode("head");
  if (head) {
    const phase = elapsed % 18;
    const glance = moving && phase > 9 && phase < 13 ? Math.sin((phase - 9) * Math.PI / 4) ** 2 : 0;
    head.rotation.x = emotions.embarrassment / 100 * 0.08 - emotions.surprise / 100 * 0.05;
    head.rotation.y = emotions.embarrassment / 100 * 0.12 + glance * 0.09;
    head.rotation.z = emotions.curiosity / 100 * 0.08 + (thinking ? 0.12 : 0) + glance * 0.07 + (moving ? Math.sin(elapsed * (thinking ? 1.5 : 1)) * 0.025 : 0);
  }
}
