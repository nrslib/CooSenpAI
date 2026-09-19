import { useEffect, useState, type ReactElement } from "react";

import { AvatarBlob } from "../components/AvatarBlob.js";
import { CompanionModelControls } from "../components/CompanionModelControls.js";
import { CloseIcon } from "../components/LineIcons.js";
import { applyAppearance } from "../appearance.js";
import { modelPopupApi, setIpcLocale } from "../ipc.js";
import { modelPopupKeyAction } from "./state.js";
import type { ModelPickerView } from "./view.js";
import { I18nProvider, t, type Locale } from "../i18n/index.js";

export function ModelPopup(): ReactElement {
  const [view, setView] = useState<ModelPickerView>();
  const [error, setError] = useState<string>();
  const locale: Locale = view?.language ?? "ja";

  useEffect(() => {
    const controls = modelPopupApi.subscribeView((next) => {
      setView(next);
      setError(undefined);
    });
    void controls.ready.then(() => modelPopupApi.ready()).then((result) => { if (!result.ok) setError(result.error.message); });
    return () => { controls.dispose(); };
  }, []);

  useEffect(() => {
    if (view === undefined) return;
    setIpcLocale(view.language);
    applyAppearance({ theme: view.theme, font: view.font });
  }, [view?.language, view?.theme, view?.font]);

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
          <AvatarBlob color={view?.avatarColor} image={view?.avatarImagePng ?? undefined} size={24} />
          <div><span>{t(locale, "modelPopup.title")}</span><small>{view?.companionDisplayName ?? t(locale, "modelPopup.companionDefault")}</small></div>
        </div>
        <button className="model-popup-close" type="button" aria-label={t(locale, "modelPopup.close")} onClick={close}><CloseIcon /></button>
      </header>
      {view === undefined
        ? <p className="model-popup-status" role={error === undefined ? undefined : "alert"}>{error ?? t(locale, "common.loading")}</p>
        : <>
          <CompanionModelControls view={view} onEvent={(event) => { void modelPopupApi.input(event); }} />
          {(view.error ?? error) != null ? <p className="model-popup-error" role="alert">{view.error ?? error}</p> : null}
        </>}
    </section>
  </main></I18nProvider>;
}
