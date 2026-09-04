import type { AppSnapshot, IpcResult, SnapshotEvent } from "../types.js";

export interface ModelPopupViewState {
  readonly snapshot?: AppSnapshot;
  readonly revision: number;
}

export const initialModelPopupViewState: ModelPopupViewState = { revision: 0 };

interface ModelPopupSnapshotSubscription {
  readonly ready: Promise<void>;
  readonly dispose: () => void;
}

interface ModelPopupSnapshotApi {
  readonly subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => ModelPopupSnapshotSubscription;
  readonly getSnapshot: () => Promise<IpcResult<AppSnapshot>>;
}

export function applyModelPopupSnapshot(
  state: ModelPopupViewState,
  snapshot: AppSnapshot,
): ModelPopupViewState {
  if (snapshot.revision <= state.revision) return state;
  return { snapshot, revision: snapshot.revision };
}

export function connectModelPopupSnapshots(
  api: ModelPopupSnapshotApi,
  apply: (snapshot: AppSnapshot) => void,
  onError: (message: string) => void,
): () => void {
  let active = true;
  const subscription = api.subscribeSnapshots((event) => {
    if (active) apply(event.snapshot);
  });
  void subscription.ready.then(() => api.getSnapshot()).then((result) => {
    if (!active) return;
    if (result.ok) apply(result.value);
    else onError(result.error.message);
  });
  return () => {
    active = false;
    subscription.dispose();
  };
}

export function modelPopupKeyAction(key: string): "close" | "ignore" {
  return key === "Escape" ? "close" : "ignore";
}

export function modelPopupControlsDisabled(snapshot: Pick<AppSnapshot, "onboarding">): boolean {
  return snapshot.onboarding.tutorialActive;
}
