import { useEffect, useLayoutEffect, useState, type ReactElement } from "react";
import { applyAppearance } from "../appearance.js";
import { brainActivityApi, ipcTransportError, setIpcLocale } from "../ipc.js";
import { I18nProvider, t, type Locale } from "../i18n/index.js";
import type { AppSnapshot } from "../types.js";
import { BrainActivityPanel } from "./BrainActivityPanel.js";
import "./brain-activity.css";

export function BrainActivityWindow(): ReactElement {
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const locale: Locale = snapshot?.config.ui.language === "en" ? "en" : "ja";
  useLayoutEffect(() => {
    setIpcLocale(locale);
    document.documentElement.lang = locale;
    document.title = `CooSenpAI — ${t(locale, "brain.title")}`;
    if (snapshot !== null) applyAppearance({ theme: snapshot.config.ui.theme, font: snapshot.config.ui.font });
  }, [snapshot, locale]);
  useEffect(() => {
    let active = true;
    let revision = -1;
    const subscription = brainActivityApi.subscribeView((value) => {
      if (!active || (value !== null && value.revision < revision)) return;
      if (value !== null) revision = value.revision;
      setSnapshot(value);
    });
    void subscription.ready.then(async () => {
      if (!active) return;
      const result = await brainActivityApi.ready();
      if (active && !result.ok) setError(result.error.message);
    }).catch((cause: unknown) => { if (active) setError(ipcTransportError(cause)); });
    return () => { active = false; subscription.dispose(); };
  }, []);
  const close = async (): Promise<void> => {
    const result = await brainActivityApi.close();
    if (!result.ok) setError(result.error.message);
  };
  return <I18nProvider locale={locale}><main className="brain-window">
    <header className="brain-window-header"><h1>{t(locale, "brain.title")}</h1><button type="button" onClick={() => { void close(); }}>{t(locale, "brain.close")}</button></header>
    {error === null ? null : <p role="alert">{error}</p>}
    {snapshot === null ? <p role="status">{t(locale, "common.loading")}</p> : <BrainActivityPanel decision={snapshot.latestJudgeDecision} enabled={(snapshot.config.judge?.modules?.length ?? 0) > 0} sourceKey={JSON.stringify(snapshot.config.judge)} />}
  </main></I18nProvider>;
}
