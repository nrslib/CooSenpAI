import { useState, type ReactElement } from "react";
import type { CompanionEmotions } from "../types.js";
import { useI18n } from "../i18n/index.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";
import { companionEmotionKeys, companionEmotionLabel } from "../companion-emotions.js";

export function CompanionEmotionsPanel({ displayName, emotions, enabled, onReset, pending = false, error, open }: { readonly displayName: string; readonly emotions: CompanionEmotions; readonly enabled: boolean; readonly onReset: () => void; readonly pending?: boolean; readonly error?: string; readonly open?: boolean }): ReactElement {
  const { locale, t } = useI18n();
  const [confirming, setConfirming] = useState(false);
  return <>
    <details className="companion-emotions" open={open}><summary>{t("emotions.summary", { name: displayName })}</summary>
      {enabled ? null : <p>{t("emotions.disabled")}</p>}
      <dl>{companionEmotionKeys.map((key) => <div key={key}><dt>{companionEmotionLabel(key, locale)}</dt><dd>{emotions[key]} / 100</dd></div>)}</dl>
      <p>{t("emotions.resetDescription")}</p>
      <button type="button" disabled={pending} onClick={() => setConfirming(true)}>{pending ? t("emotions.resetting") : t("emotions.reset")}</button>
      {error === undefined ? null : <p role="alert">{error}</p>}
    </details>
    {confirming ? <ConfirmationDialog id="emotions-reset" title={t("emotions.resetTitle")} description={t("emotions.resetDialogDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("emotions.confirmReset")} onCancel={() => setConfirming(false)} onConfirm={() => { setConfirming(false); onReset(); }} /> : null}
  </>;
}
