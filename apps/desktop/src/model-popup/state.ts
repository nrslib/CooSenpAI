import type { AppSnapshot } from "../types.js";

export interface ModelPopupViewState {
  readonly snapshot?: AppSnapshot;
  readonly revision: number;
}

export const initialModelPopupViewState: ModelPopupViewState = { revision: 0 };

export function applyModelPopupSnapshot(
  state: ModelPopupViewState,
  snapshot: AppSnapshot,
): ModelPopupViewState {
  if (snapshot.revision <= state.revision) return state;
  return { snapshot, revision: snapshot.revision };
}

export function modelPopupKeyAction(key: string): "close" | "ignore" {
  return key === "Escape" ? "close" : "ignore";
}
