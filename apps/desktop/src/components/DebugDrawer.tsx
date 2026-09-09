import type { ReactElement } from "react";

import type { DebugDetail } from "../types.js";
import { useI18n } from "../i18n/index.js";

export function DebugDrawer({ detail, close }: { readonly detail: DebugDetail; readonly close: () => void }): ReactElement {
  const { t } = useI18n();
  return <aside className="debug-drawer">
    <div className="drawer-heading"><h3>{t("debug.title")}</h3><button type="button" onClick={close}>{t("debugDrawer.close")}</button></div>
    <p>{t("debug.images", { value: detail.imageFiles.length === 0 ? t("debugDrawer.none") : detail.imageFiles.join(", ") })}</p>
    <p>{t("debug.ocr", { value: detail.ocrPreview ?? t("debugDrawer.none") })}</p>
    <h4>{t("debug.response")}</h4>
    <pre>{detail.observerResponse === undefined ? t("debugDrawer.none") : JSON.stringify(detail.observerResponse, null, 2)}</pre>
    <h4>{t("debug.context")}</h4>
    <pre>{detail.companionContext ?? t("debugDrawer.none")}</pre>
  </aside>;
}
