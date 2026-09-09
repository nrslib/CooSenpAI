import type { ReactElement } from "react";
import { useI18n } from "../i18n/index.js";
import type { VoiceOutputSnapshot } from "../voice-output.js";
import { useStatusPresenter } from "../useStatusPresenter.js";

interface Props {
  readonly enabled: boolean;
  readonly showTest: boolean;
}

export function VoiceOutputPanel({ enabled, showTest }: Props): ReactElement | null {
  const { t } = useI18n();
  const presenter = useStatusPresenter<VoiceOutputSnapshot>("voice", enabled, showTest);
  const state = presenter.state;
  if (state === undefined) return presenter.error === undefined ? null : <p role="alert">{presenter.error}</p>;
  if (!state.visible) return null;
  const snapshot = state.snapshot ?? undefined;
  const error = presenter.error ?? state.error ?? undefined;
  return <section className="voice-output-panel" aria-label={t("voiceOutput.label")}>
    <p role="status" aria-live="polite">{snapshot === undefined ? t("voiceOutput.checking") : snapshot.speaking ? t("voiceOutput.speaking") : t("voiceOutput.stopped")}</p>
    {snapshot?.message && <p role="alert" className="field-error">{snapshot.message}</p>}
    {error !== undefined && error !== snapshot?.message && <p role="alert" className="field-error">{error}</p>}
    <div className="button-row">
      {showTest && <button type="button" disabled={!state.canTest} onClick={() => presenter.action("test")}>{t("voiceOutput.test")}</button>}
      <button type="button" disabled={!state.canStop} onClick={() => presenter.action("stop")}>{t("voiceOutput.stop")}</button>
    </div>
    {showTest && !enabled && <p className="field-help">{t("voiceOutput.disabledHelp")}</p>}
  </section>;
}
