import type { BubbleRecord } from "../types.js";
export interface BubbleControlsView {
  readonly record: BubbleRecord | null;
  readonly exiting: boolean;
  readonly reading: boolean;
  readonly busy: boolean;
  readonly dismissible: boolean;
  readonly bodyButton: boolean;
  readonly olderEdge: boolean;
  readonly secondEdge: boolean;
  readonly showLatest: boolean;
  readonly navigationDisabled: boolean;
  readonly requiredInput: boolean;
  readonly error: string | null;
  readonly clearSecret: number;
  readonly focusRequest: number;
}
export type BubbleViewInput =
  | { readonly type: "pointer" | "body"; readonly id: string; readonly control: boolean }
  | { readonly type: "key"; readonly id: string; readonly key: string; readonly composing: boolean; readonly keyCode: number; readonly control: boolean }
  | { readonly type: "dismiss"; readonly id: string }
  | { readonly type: "select" | "secret"; readonly id: string; readonly value: string }
  | { readonly type: "action"; readonly id: string; readonly action: string }
  | { readonly type: "navigate"; readonly direction: "older" | "newer" | "latest" }
  | { readonly type: "wheel"; readonly delta: number; readonly control: boolean; readonly scrollTop: number; readonly clientHeight: number; readonly scrollHeight: number };
