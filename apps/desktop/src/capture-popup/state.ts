import type { AppSnapshot, CapturePopupSnapshot, SnapshotEvent } from "../types.js";

type CompanionSnapshotEvent = Pick<SnapshotEvent, "revision"> & {
  readonly snapshot: Pick<AppSnapshot, "companionDisplayName" | "config" | "avatarImagePng">;
};

export interface CapturePopupViewState {
  readonly snapshot?: CapturePopupSnapshot;
  readonly revision: number;
  readonly companionDisplayName?: string;
  readonly expectedCaptureId?: string;
}

export const initialCapturePopupState: CapturePopupViewState = { revision: 0 };

export function captureHasSendableAttachment(
  snapshot: Pick<CapturePopupSnapshot, "attachmentKind" | "textPreview"> | undefined,
): boolean {
  return snapshot !== undefined
    && (snapshot.attachmentKind !== "text" || snapshot.textPreview !== undefined);
}

export function applyCaptureSnapshot(
  state: CapturePopupViewState,
  snapshot: CapturePopupSnapshot,
): CapturePopupViewState {
  if (state.expectedCaptureId !== undefined && snapshot.captureId !== state.expectedCaptureId) {
    return state;
  }
  return {
    snapshot,
    revision: Math.max(state.revision, snapshot.revision),
    companionDisplayName: state.revision > snapshot.revision
      ? state.companionDisplayName ?? snapshot.companionDisplayName
      : snapshot.companionDisplayName,
    expectedCaptureId: snapshot.captureId,
  };
}

export function expectCapture(
  state: CapturePopupViewState,
  captureId: string,
): CapturePopupViewState {
  return { ...state, snapshot: undefined, expectedCaptureId: captureId };
}

export function applySnapshotEvent(
  state: CapturePopupViewState,
  event: CompanionSnapshotEvent,
): CapturePopupViewState {
  if (event.revision <= state.revision) return state;
  const companionDisplayName = event.snapshot.companionDisplayName;
  const { theme, font } = event.snapshot.config.ui;
  return {
    snapshot: state.snapshot === undefined
      ? undefined
      : {
        ...state.snapshot,
        revision: event.revision,
        companionDisplayName,
        theme,
        font,
        avatarImagePng: event.snapshot.avatarImagePng,
      },
    revision: event.revision,
    companionDisplayName,
    expectedCaptureId: state.expectedCaptureId,
  };
}
