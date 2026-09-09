import { useEffect, useState, type ReactElement } from "react";

import { AvatarBlob } from "../components/AvatarBlob.js";
import { CompanionModelControls } from "../components/CompanionModelControls.js";
import { CloseIcon } from "../components/LineIcons.js";
import { applyAppearance } from "../appearance.js";
import { modelPopupApi, setIpcLocale } from "../ipc.js";
import type { AppSnapshot } from "../types.js";
import { applyModelPopupSnapshot, initialModelPopupViewState, modelPopupKeyAction } from "./state.js";
import type { ModelPickerView } from "./view.js";
import { I18nProvider, t, type Locale } from "../i18n/index.js";

export function ModelPopup(): ReactElement {
  const [viewState, setViewState] = useState(initialModelPopupViewState);
  const [view, setView] = useState<ModelPickerView>();
  const [error, setError] = useState<string>();
  const snapshot = viewState.snapshot;
  const locale: Locale = snapshot?.config.ui.language ?? "ja";

  useEffect(() => {
    const apply = (next: AppSnapshot): void => {
      setIpcLocale(next.config.ui.language);
      setViewState((current) => applyModelPopupSnapshot(current, next));
      setError(undefined);
    };
    const snapshots = modelPopupApi.subscribeSnapshots((event) => apply(event.snapshot));
    const controls = modelPopupApi.subscribeView(setView);
    void Promise.all([snapshots.ready, controls.ready]).then(() => modelPopupApi.ready()).then((result) => { if (!result.ok) setError(result.error.message); });
    return () => { snapshots.dispose(); controls.dispose(); };
  }, []);

  useEffect(() => {
    if (snapshot === undefined) return;
    applyAppearance({ theme: snapshot.config.ui.theme, font: snapshot.config.ui.font });
  }, [snapshot?.config.ui.font, snapshot?.config.ui.theme]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (modelPopupKeyAction(event.key) !== "close") return;
      event.preventDefault();
      void modelPopupApi.close();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, []);

  const close = (): void => {
    void modelPopupApi.close().then((result) => {
      if (!result.ok) setError(result.error.message);
    });
  };

  return <I18nProvider locale={locale}><main className="model-popup-shell">
    <section className="model-popup-card" aria-label={t(locale, "modelPopup.title")}>
      <header className="model-popup-header">
        <div className="model-popup-title">
          <AvatarBlob color={snapshot?.config.ui.avatarColor} image={snapshot?.avatarImagePng} size={24} />
          <div><span>{t(locale, "modelPopup.title")}</span><small>{snapshot?.companionDisplayName ?? t(locale, "modelPopup.companionDefault")}</small></div>
        </div>
        <button className="model-popup-close" type="button" aria-label={t(locale, "modelPopup.close")} onClick={close}><CloseIcon /></button>
      </header>
      {snapshot === undefined
        ? <p className="model-popup-status" role={error === undefined ? undefined : "alert"}>{error ?? t(locale, "common.loading")}</p>
        : view === undefined ? null : <CompanionModelControls view={view} onEvent={(event) => { void modelPopupApi.input(event); }} />}
      {snapshot !== undefined && (view?.error || error) ? <p className="model-popup-error" role="alert">{view?.error || error}</p> : null}
    </section>
  </main></I18nProvider>;
}
