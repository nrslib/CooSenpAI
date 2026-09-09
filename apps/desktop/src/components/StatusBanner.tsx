import type { ReactElement } from "react";
import { useI18n } from "../i18n/index.js";
import { renderUiText, type BannerView } from "../app-view.js";
export function StatusBanner({ view, onRecover }: { readonly view: BannerView | null; readonly onRecover: () => void }): ReactElement | null {
  const { t } = useI18n();
  if (view === null) return null;
  return <div className={`status-banner tone-${view.tone}`} role={view.tone === "error" ? "alert" : "status"}>
    <span>{renderUiText(view.message, t)}</span>
    {view.actionLabel === null ? null : <button type="button" onClick={onRecover}>{renderUiText(view.actionLabel, t)}</button>}
  </div>;
}
