import type { AvatarSnapshot } from "./state.js";
import type { CompanionEmotions } from "../types.js";
import type { UiText } from "../app-view.js";
import type { MotionSlot } from "../vrm/motion-settings.js";
export interface AvatarSceneView {
  readonly snapshot: AvatarSnapshot; readonly generation: number; readonly load: boolean; readonly status: UiText | null; readonly retryable: boolean;
  readonly replyDirective: number; readonly replyAction: "none" | "reply" | "cancel"; readonly previewDirective: number;
  readonly preview: { readonly id: number; readonly slot: MotionSlot } | null; readonly dragRequest: number; readonly emotions: CompanionEmotions;
}
export type AvatarSceneInput = { readonly type: "mounted" | "visibility"; readonly hidden: boolean }
  | { readonly type: "unmounted" | "retry" | "hide" }
  | { readonly type: "drag"; readonly button: number }
  | { readonly type: "operationFailed"; readonly error: string }
  | { readonly type: "loaded"; readonly generation: number; readonly outcome: "ready" | "unset" }
  | { readonly type: "failed"; readonly generation: number; readonly error: string }
  | { readonly type: "previewApplied"; readonly generation: number; readonly requestId: number };
