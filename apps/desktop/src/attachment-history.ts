import type { ConversationEntry } from "./types.js";

export interface AttachmentHistoryItem {
  readonly input: ConversationEntry;
  readonly reply?: ConversationEntry;
  readonly kind: "image" | "text";
}

export function attachmentHistory(entries: readonly ConversationEntry[]): readonly AttachmentHistoryItem[] {
  const replies = new Map<string, ConversationEntry>();
  for (const entry of entries) {
    if (entry.role !== "companion") continue;
    const causes = entry.causedByIds ?? [];
    for (const id of causes) {
      if (!replies.has(id)) replies.set(id, entry);
    }
  }
  return entries
    .filter((entry) => entry.role === "user" && (entry.attachmentPath !== undefined || entry.attachmentText !== undefined))
    .map((input) => ({
      input,
      reply: replies.get(input.id),
      kind: input.attachmentPath === undefined ? "text" as const : "image" as const,
    }))
    .reverse();
}
