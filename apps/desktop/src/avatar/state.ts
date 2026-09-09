import type { CompanionEmotions } from "../types.js";

export interface AvatarSnapshot {
  readonly revision: number;
  readonly modelRevision: number;
  readonly language: "ja" | "en";
  readonly visible: boolean;
  readonly emotions: CompanionEmotions;
  readonly emotionsEnabled: boolean;
  readonly thinking: boolean;
  readonly replyId: string | null;
}
