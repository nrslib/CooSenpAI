import type { UiText } from "../app-view.js";
import type { MotionSlot } from "./motion-settings.js";
export type MotionSelectionView = { readonly kind: "builtin"; readonly id: "idle" | "reply" } | { readonly kind: "file"; readonly name: string } | { readonly kind: "none" };
export interface MotionView { readonly settings: Readonly<Record<MotionSlot, MotionSelectionView>> | null; readonly busy: boolean; readonly error: UiText | null; readonly status: UiText | null; readonly previewable: readonly MotionSlot[] }
export type MotionInput = { readonly type: "mounted" | "unmounted" }
  | { readonly type: "select"; readonly slot: MotionSlot; readonly value: string }
  | { readonly type: "reset" | "preview"; readonly slot: MotionSlot }
  | { readonly type: "import"; readonly slot: MotionSlot; readonly fileId: string; readonly name: string; readonly size: number; readonly consent: boolean }
  | { readonly type: "storageLoaded"; readonly generation: number; readonly settings: MotionView["settings"]; readonly error: string | null }
  | { readonly type: "storageSaved"; readonly generation: number; readonly error: string | null };
export type MotionStorageCommand = { readonly kind: "load"; readonly generation: number }
  | { readonly kind: "save"; readonly generation: number; readonly slot: MotionSlot; readonly selection: Exclude<MotionSelectionView, {kind: "file"}> }
  | { readonly kind: "import"; readonly generation: number; readonly slot: MotionSlot; readonly fileId: string }
  | { readonly kind: "release"; readonly fileId: string };
