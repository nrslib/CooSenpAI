import type { ReactElement } from "react";

interface StartupProps {
  readonly error: string;
  readonly retrying: boolean;
  readonly onRetry: () => void;
  readonly onSettings: () => void;
  readonly onExit: () => void;
}

import { useI18n } from "../i18n/index.js";

export function StartupRecoveryScreen({ error, retrying, onRetry, onSettings, onExit }: StartupProps): ReactElement {
  const { t } = useI18n();
  return <main className="recovery-screen" data-testid="startup-recovery">
    <section className="recovery-card">
      <h1>{t("recovery.startupTitle")}</h1>
      <p role="alert">{error}</p>
      <div className="button-row">
        <button type="button" disabled={retrying} onClick={onRetry}>{retrying ? t("common.retrying") : t("common.retry")}</button>
        <button type="button" className="secondary" onClick={onSettings}>{t("common.openSettings")}</button>
        <button type="button" className="secondary" onClick={onExit}>{t("common.exit")}</button>
      </div>
      <small>{t("recovery.stopDescription")}</small>
    </section>
  </main>;
}

export function FinishRecoveryScreen({ error, busy, onRetry }: { readonly error?: string; readonly busy: boolean; readonly onRetry: () => void }): ReactElement {
  const { t } = useI18n();
  return <section className="finish-recovery" data-testid="finish-recovery">
    <h1>{t("recovery.finishTitle")}</h1>
    <p>{t("recovery.finishDescription")}</p>
    {error === undefined ? null : <p className="error-text" role="alert">{error}</p>}
    <button type="button" disabled={busy} onClick={onRetry}>{busy ? t("common.retrying") : t("app.tutorialFinishRetry")}</button>
  </section>;
}
