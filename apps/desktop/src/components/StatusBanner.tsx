import type { ReactElement } from "react";

import type { AppSnapshot } from "../types.js";
import { statusBanner } from "../view-model.js";

interface Props {
  readonly snapshot: AppSnapshot;
  readonly transientError?: string;
  readonly onOpenSettings: () => void;
  readonly onOpenSystemSettings: () => void;
  readonly onRelaunch: () => void;
  readonly onOpenSpeechSettings: () => void;
}

export function StatusBanner({ snapshot, transientError, onOpenSettings, onOpenSystemSettings, onRelaunch, onOpenSpeechSettings }: Props): ReactElement | null {
  const banner = statusBanner(snapshot);
  if (transientError !== undefined) {
    return <div className="status-banner tone-error" role="alert"><span>{transientError}</span></div>;
  }
  if (banner === undefined) return null;
  const action = banner.action === "open-speech-settings"
    ? onOpenSpeechSettings
    : banner.action === "open-settings"
    ? onOpenSystemSettings
    : banner.action === "open-app-settings"
      ? onOpenSettings
      : banner.action === "relaunch"
        ? onRelaunch
        : undefined;
  return <div className={`status-banner tone-${banner.tone}`} role={banner.tone === "error" ? "alert" : "status"}>
    <span>{banner.message}</span>
    {action === undefined ? null : <button type="button" onClick={action}>{banner.actionLabel}</button>}
  </div>;
}
