import { useCallback, useEffect, useLayoutEffect, useState, type ReactElement } from "react";

import { AppUpdatePanel } from "./components/AppUpdatePanel.js";
import { VoiceOutputPanel } from "./components/VoiceOutputPanel.js";
import { Conversation } from "./components/Conversation.js";
import { AttachmentHistory } from "./components/AttachmentHistory.js";
import { createHeaderMenuItems, Header } from "./components/Header.js";
import { NowLine } from "./components/NowLine.js";
import { StatusBanner } from "./components/StatusBanner.js";
import { FinishRecoveryScreen, StartupRecoveryScreen } from "./components/RecoveryScreen.js";
import { ThoughtBubble } from "./components/ThoughtBubble.js";
import { CloseIcon } from "./components/LineIcons.js";
import { ConfirmationDialog } from "./components/ConfirmationDialog.js";
import { avatarApi, desktopApi, setIpcLocale } from "./ipc.js";
import { SettingsPanel } from "./SettingsPanel.js";
import type { SettingsAppearancePreview } from "./settings-form.js";
import type { AppSnapshot, CooSenpaiConfig, IpcResult, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "./types.js";
import { renderUiText, type AppInput, type AppView, type StatusView } from "./app-view.js";
import { appearanceFromConfig, applyAppearance } from "./appearance.js";
import { I18nProvider, t, type Locale } from "./i18n/index.js";

export function App(): ReactElement {
  const [snapshot, setSnapshot] = useState<AppSnapshot>();
  const [view, setView] = useState<AppView>();
  const [status, setStatus] = useState<StatusView>();
  const [personas, setPersonas] = useState<readonly PersonaOption[]>([]);
  const [providerModels, setProviderModels] = useState<readonly ProviderModelOptions[]>([]);
  const [providerModelsError, setProviderModelsError] = useState<string>();
  const [providerApiKeys, setProviderApiKeys] = useState<ProviderApiKeyStatus>();
  const [providerApiKeysError, setProviderApiKeysError] = useState<string>();
  const send = (event: AppInput): void => { void desktopApi.appInput(event); };
  const [appearancePreview, setAppearancePreview] = useState<SettingsAppearancePreview>();

  useEffect(() => {
    if (appearancePreview !== undefined) {
      applyAppearance({ theme: appearancePreview.theme, font: appearancePreview.font });
    } else if (snapshot !== undefined) applyAppearance(appearanceFromConfig(snapshot.config));
  }, [appearancePreview, snapshot?.config.ui.font, snapshot?.config.ui.theme]);

  useLayoutEffect(() => {
    if (view !== undefined && view.focusRequest > 0) document.querySelector<HTMLTextAreaElement>(".composer textarea")?.focus();
  }, [view?.focusRequest]);
  useLayoutEffect(() => {
    if (view !== undefined) send({ type: "settingsPresented", generation: view.settingsGeneration });
  }, [view?.settings, view?.settingsGeneration]);

  const previewAppearance = useCallback((preview?: SettingsAppearancePreview): Promise<IpcResult<null>> => {
    setAppearancePreview(preview);
    return desktopApi.previewSettingsAppearance(preview);
  }, []);

  useEffect(() => {
    let mounted = true;
    const snapshots = desktopApi.subscribeSnapshots(({ snapshot: next }) => {
      setIpcLocale(next.config.ui.language);
      setSnapshot((current) => current === undefined || next.revision > current.revision ? next : current);
    });
    const app = desktopApi.subscribeAppView(setView);
    const status = desktopApi.subscribeStatusView(setStatus);
    const personas = desktopApi.subscribePersonas(setPersonas);
    const resources = desktopApi.subscribeSettingsResources(({ models, keys }) => {
      if (models.ok) { setProviderModels(models.value); setProviderModelsError(undefined); }
      else setProviderModelsError(models.error.message);
      if (keys.ok) { setProviderApiKeys(keys.value); setProviderApiKeysError(undefined); }
      else setProviderApiKeysError(keys.error.message);
    });
    void Promise.all([snapshots.ready, app.ready, status.ready, personas.ready, resources.ready]).then(() => {
      if (mounted) void desktopApi.ready();
    });
    return () => { mounted = false; snapshots.dispose(); app.dispose(); status.dispose(); personas.dispose(); resources.dispose(); };
  }, []);

  if (view?.screen === "startupError") return <I18nProvider locale="ja"><StartupRecoveryScreen
    error={view.error ?? ""} retrying={view.loading}
    onRetry={() => send({ type: "retry" })} onSettings={() => send({ type: "startupSettings" })} onExit={() => { void desktopApi.exit(); }}
  /></I18nProvider>;
  if (snapshot === undefined || view === undefined || view.screen === "loading") return <I18nProvider locale="ja"><main className="loading-screen"><div className="loading-orb" /><p>{t("ja", "app.loading")}</p></main></I18nProvider>;

  const locale: Locale = snapshot.config.ui.language;

  const handle = async <T,>(promise: Promise<IpcResult<T>>): Promise<IpcResult<T>> => {
    const result = await promise;
    send({ type: "report", error: result.ok ? null : result.error.message });
    return result;
  };
  const selectPersona = (persona: string): Promise<IpcResult<CooSenpaiConfig>> => desktopApi.appSelectPersona(persona);
  const refreshPersonas = async (): Promise<IpcResult<readonly PersonaOption[]>> => {
    const result = await desktopApi.listPersonas();
    if (result.ok) setPersonas(result.value);
    return result;
  };
  const saveProviderApiKey = async (provider: ProviderName, apiKey: string): Promise<IpcResult<ProviderApiKeyStatus>> => {
    const result = await handle(desktopApi.setProviderApiKey(provider, apiKey));
    if (result.ok) {
      setProviderApiKeys(result.value);
      setProviderApiKeysError(undefined);
    } else {
      setProviderApiKeysError(result.error.message);
    }
    return result;
  };
  const deleteProviderApiKey = async (provider: ProviderName): Promise<IpcResult<ProviderApiKeyStatus>> => {
    const result = await handle(desktopApi.deleteProviderApiKey(provider));
    if (result.ok) {
      setProviderApiKeys(result.value);
      setProviderApiKeysError(undefined);
    } else {
      setProviderApiKeysError(result.error.message);
    }
    return result;
  };
  const requestSettings = (): void => send({ type: "openSettings" });
  const headerMenuItems = createHeaderMenuItems(
    () => send({ type: "menuModel" }), () => send({ type: "menuReset" }), view.canReset, locale,
  );
  const text = (value: import("./app-view.js").UiText) => renderUiText(value, (key, args) => t(locale, key, args));
  const renderedSnapshot = appearancePreview === undefined ? snapshot : {
    ...snapshot,
    config: {
      ...snapshot.config,
      ui: { ...snapshot.config.ui, theme: appearancePreview.theme, font: appearancePreview.font, avatarColor: appearancePreview.avatarColor },
      bubble: { ...snapshot.config.bubble, position: appearancePreview.bubblePosition, display: appearancePreview.bubbleDisplay },
    },
  };

  return <I18nProvider locale={locale}><main className="app-shell">
    {view.screen === "finish" ? <FinishRecoveryScreen error={view.error ?? undefined} busy={view.finishBusy} onRetry={() => send({ type: "tutorialFinish" })} /> : view.screen === "setup" ? null : <>
      <Header
      snapshot={renderedSnapshot}
      presence={status?.presence ?? { mode: "resting", text: { kind: "literal", text: "" } }}
      menuOpen={view.menuOpen} onMenuToggle={() => send({ type: "menuToggle" })} onMenuDismiss={() => send({ type: "menuDismiss" })}
      watchIntentActive={snapshot.watchIntentActive}
      watchChanging={view.watchChanging}
      onToggleWatch={() => send({ type: "toggleWatch" })}
      audioEnabled={snapshot.config.audio.enabled}
      audioChanging={view.audioChanging}
      onToggleAudio={() => send({ type: "toggleAudio" })}
      onOpenSettings={requestSettings}
      historyOpen={view.historyOpen}
      onToggleHistory={() => send({ type: "toggleHistory" })}
      menuItems={headerMenuItems}
    />
    <NowLine snapshot={renderedSnapshot} onOpenDetails={() => { void handle(desktopApi.openDetails()); }} onAssertiveness={(value) => { void handle(desktopApi.setAssertiveness(value)); }} />
    <AppUpdatePanel enabled={snapshot.config.app.checkForUpdates} showCheck={false} />
    <VoiceOutputPanel enabled={snapshot.config.voiceOutput.enabled} showTest={false} />
    <StatusBanner view={status?.banner ?? null} onRecover={() => send({ type: "recover" })} />
    <ThoughtBubble view={status?.thought ?? null} />
    {view.tutorial === null ? null : <div className="tutorial-controls" role="status"><span>{text(view.tutorial.message)}</span>
      {view.tutorial.next === null ? null : <button type="button" onClick={() => send({ type: "tutorialNext" })}>{t(locale, view.tutorial.next === "retry" ? "app.tutorialRetry" : "app.tutorialNext")}</button>}
      <button type="button" disabled={view.finishBusy} onClick={() => send({ type: "tutorialFinish" })}>{text(view.tutorial.finishLabel)}</button></div>}
      {view.historyOpen ? <AttachmentHistory snapshot={renderedSnapshot} /> : <Conversation
      snapshot={renderedSnapshot}
      onInputActive={(active) => { void desktopApi.setChatInputActive(active); }}
      onRead={() => { void desktopApi.markUnreadRead(); }}
      />}
    </>}
    {view.settings === "locked" ? <div className="settings-overlay"><section className="settings-panel tutorial-locked-settings" aria-label={t(locale, "settings.label")}><div className="settings-heading"><div><span>{t(locale, "settings.label")}</span><h2>CooSenpAI</h2></div><button className="icon-button" type="button" aria-label={t(locale, "common.close")} onClick={() => { send({ type: "closeSettings" }); }}><CloseIcon /></button></div><p>{t(locale, "app.setupLockedSettings")}</p></section></div> : view.settings === "open" ? <SettingsPanel
      snapshot={snapshot}
      personas={personas}
      providerModels={providerModels}
      providerModelsError={providerModelsError}
      providerApiKeys={providerApiKeys}
      providerApiKeysError={providerApiKeysError}
      focusSection={view.settingsFocus ?? undefined}
      onClose={() => { send({ type: "closeSettings" }); }}
      onSave={(patch, avatarImage, baseConfigRevision) => handle(desktopApi.updateConfig(patch, avatarImage, baseConfigRevision))}
      onSelectPersona={selectPersona}
      onReloadConfig={() => handle(desktopApi.getPersistedConfig())}
      onReloadPersona={() => handle(desktopApi.reloadPersona())}
      onGetPersona={(id) => desktopApi.getPersona(id)}
      onSavePersona={(id, displayName, body) => handle(desktopApi.savePersona(id, displayName, body))}
      onDeletePersona={(id) => handle(desktopApi.deletePersona(id))}
      onRefreshPersonas={refreshPersonas}
      onRestorePersona={(id, version) => handle(desktopApi.restorePersona(id, version))}
      onRestartTutorial={() => handle(desktopApi.restartTutorial())}
      onRestartSetup={() => handle(desktopApi.restartSetup())}
      onResetConversation={() => handle(desktopApi.resetConversation())}
      onOpenSystemSettings={() => void handle(desktopApi.openSystemSettings())}
      onOpenLicenseDocument={(document) => void handle(desktopApi.openLicenseDocument(document))}
      onOpenSpeechSettings={(kind) => void handle(desktopApi.openSpeechSettings(kind))}
      onToggleAvatar={() => void handle(avatarApi.toggle())}
      onRelaunch={() => void handle(desktopApi.relaunch())}
      onAppearancePreview={previewAppearance}
      onSaveProviderApiKey={saveProviderApiKey}
      onDeleteProviderApiKey={deleteProviderApiKey}
    /> : null}
    {view.resetConfirmOpen ? <ConfirmationDialog id="conversation-reset" title={t(locale, "app.resetConversationTitle")} description={t(locale, "app.resetConversationDescription")} cancelLabel={t(locale, "common.cancel")} confirmLabel={t(locale, "app.resetConversation")} onCancel={() => send({ type: "resetCancel" })} onConfirm={() => send({ type: "resetConfirm" })} /> : null}
  </main></I18nProvider>;
}
