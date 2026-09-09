import type { BubbleSnapshot } from "../types.js";

export type BubbleUpdateDecision = "apply" | "appearance" | "ignore";

export function classifyBubbleUpdate(currentGeneration: number, incoming: BubbleSnapshot): BubbleUpdateDecision {
  if (incoming.generation > currentGeneration) return "apply";
  if (incoming.generation === currentGeneration) return "appearance";
  return "ignore";
}
