import type { BubbleRecord, BubbleSnapshot } from "../types.js";

export type BubbleUpdateDecision = "apply" | "appearance" | "ignore";

export function classifyBubbleUpdate(currentGeneration: number, incoming: BubbleSnapshot): BubbleUpdateDecision {
  if (incoming.generation > currentGeneration) return "apply";
  if (incoming.generation === currentGeneration) return "appearance";
  return "ignore";
}

export function stableBubbleOrder(records: readonly BubbleRecord[]): readonly BubbleRecord[] {
  return [...records];
}

export function reuseUnchangedBubbleRecords(
  previous: readonly BubbleRecord[],
  incoming: readonly BubbleRecord[],
): readonly BubbleRecord[] {
  const previousById = new Map(previous.map((record) => [record.id, record]));
  return incoming.map((record) => {
    const existing = previousById.get(record.id);
    return existing !== undefined && JSON.stringify(existing) === JSON.stringify(record)
      ? existing
      : record;
  });
}
