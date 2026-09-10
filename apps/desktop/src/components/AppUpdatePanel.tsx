import type { ReactElement } from "react";
import type { UpdateSnapshot, UpdateStatus } from "../app-update.js";
import { useStatusPresenter, type StatusView } from "../useStatusPresenter.js";
import { useI18n } from "../i18n/index.js";

interface Props {
  readonly enabled: boolean;
  readonly showCheck: boolean;
}

interface ControlsProps extends Props {
  readonly controls: StatusView<UpdateSnapshot>;
  readonly status: UpdateStatus | undefined;
  readonly busy: boolean;
  readonly error: string | undefined;
  readonly onCheck: () => void;
  readonly onInstall: () => void;
  readonly onRestart: () => void;
  readonly onLater: () => void;
}

export function UpdateControls({ enabled, showCheck, status, controls, error, onCheck, onInstall, onRestart, onLater }: ControlsProps): ReactElement | null {
  const { t } = useI18n();
  if (!controls.visible) return null;
  return <section className="app-update-panel" aria-label={t("update.label")}>
    <div role="status" aria-live="polite">
      {status?.phase === "checking" && <p>{t("update.checking")}</p>}
      {status?.phase === "upToDate" && <p>{t("update.upToDate")}</p>}
      {status?.phase === "incompatible" && <p>{t("update.incompatible", { version: status.minimumSystemVersion })}</p>}
      {status?.phase === "available" && <>
        <p>{t("update.available", { version: status.version })}</p>
        {status.notes && <details><summary>{t("update.notes")}</summary><p className="update-notes">{status.notes}</p></details>}
      </>}
      {status?.phase === "downloading" && <>
        <p>{t("update.downloading", { version: status.version })}</p>
        <progress aria-label={t("update.progress")} max={status.total !== null && status.total > 0 ? status.total : undefined} value={status.total !== null && status.total > 0 ? status.downloaded : undefined} />
      </>}
      {status?.phase === "installing" && <p>{t("update.installing", { version: status.version })}</p>}
      {status?.phase === "installed" && <p>{t("update.installed", { version: status.version })}</p>}
      {status?.phase === "failed" && <p className="field-error">{status.message}</p>}
    </div>
    {error !== undefined && !(status?.phase === "failed" && status.message === error) && <p role="alert" className="field-error">{error}</p>}
    <div className="button-row">
      {controls.showCheck && <button type="button" disabled={!controls.canCheck} onClick={onCheck}>{t("update.check")}</button>}
      {controls.showInstall && <button type="button" disabled={!controls.canInstall} onClick={onInstall}>{t("update.update")}</button>}
      {controls.showRestart && <>
        <button type="button" disabled={!controls.canRestart} onClick={onRestart}>{t("update.restart")}</button>
        {controls.showLater && <button type="button" disabled={!controls.canRestart} onClick={onLater}>{t("update.later")}</button>}
      </>}
    </div>
    {showCheck && !enabled && <p className="field-help">{t("update.disabledHelp")}</p>}
  </section>;
}

export function AppUpdatePanel({ enabled, showCheck }: Props): ReactElement | null {
  const presenter = useStatusPresenter<UpdateSnapshot>("update", enabled, showCheck);
  const controls = presenter.state;
  if (controls === undefined) return presenter.error === undefined ? null : <p role="alert">{presenter.error}</p>;
  return <UpdateControls enabled={enabled} showCheck={showCheck} controls={controls} status={controls.snapshot?.status} busy={controls.busy} error={presenter.error ?? controls.error ?? undefined}
    onCheck={() => presenter.action("check")} onInstall={() => presenter.action("install")}
    onRestart={() => presenter.action("restart")} onLater={() => presenter.action("later")} />;
}
