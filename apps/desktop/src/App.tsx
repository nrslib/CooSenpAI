import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";

import { Conversation } from "./components/Conversation.js";
import { AttachmentHistory } from "./components/AttachmentHistory.js";
import { DebugDrawer } from "./components/DebugDrawer.js";
import { createHeaderMenuItems, Header } from "./components/Header.js";
import { NowLine } from "./components/NowLine.js";
import { StatusBanner } from "./components/StatusBanner.js";
import { FinishRecoveryScreen, StartupRecoveryScreen } from "./components/RecoveryScreen.js";
import { ThoughtBubble } from "./components/ThoughtBubble.js";
import { CloseIcon } from "./components/LineIcons.js";
import { ConfirmationDialog } from "./components/ConfirmationDialog.js";
import { desktopApi } from "./ipc.js";
import { SettingsPanel } from "./SettingsPanel.js";
import type { SettingsAppearancePreview } from "./settings-form.js";
import type { AppSnapshot, DebugDetail, IpcResult, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "./types.js";
import { tutorialResponseFailed, tutorialSettingsAreAvailable, tutorialSettingsPresentationPending, tutorialStepCanBeSkipped } from "./tutorial-ui.js";
import { appearanceFromConfig, applyAppearance } from "./appearance.js";
import { createAppearancePreviewQueue } from "./settings-appearance-preview.js";

interface Selection {
  readonly id: string;
  readonly request: number;
}

export function App(): ReactElement {
  const [snapshot, setSnapshot] = useState<AppSnapshot>();
  const [error, setError] = useState<string>();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsFocus, setSettingsFocus] = useState<"watch">();
  const [selection, setSelection] = useState<Selection>();
  const [personas, setPersonas] = useState<readonly PersonaOption[]>([]);
  const [providerModels, setProviderModels] = useState<readonly ProviderModelOptions[]>([]);
  const [providerModelsError, setProviderModelsError] = useState<string>();
  const [providerApiKeys, setProviderApiKeys] = useState<ProviderApiKeyStatus>();
  const [providerApiKeysError, setProviderApiKeysError] = useState<string>();
  const [requestedWatchIntent, setRequestedWatchIntent] = useState<boolean>();
  const [requestedAudioIntent, setRequestedAudioIntent] = useState<boolean>();
  const watchRequestRevision = useRef(0);
  const audioRequestRevision = useRef(0);
  const [debugDetail, setDebugDetail] = useState<DebugDetail>();
  const [historyOpen, setHistoryOpen] = useState(false);
  const [conversationResetConfirmOpen, setConversationResetConfirmOpen] = useState(false);
  const [snapshotRetrying, setSnapshotRetrying] = useState(true);
  const [finishRetrying, setFinishRetrying] = useState(false);
  const [appearancePreview, setAppearancePreview] = useState<SettingsAppearancePreview>();
  const appearancePreviewQueue = useRef(
    createAppearancePreviewQueue(desktopApi.previewSettingsAppearance),
  );

  useEffect(() => {
    if (appearancePreview !== undefined) {
      applyAppearance({ theme: appearancePreview.theme, font: appearancePreview.font });
    } else if (snapshot !== undefined) applyAppearance(appearanceFromConfig(snapshot.config));
  }, [appearancePreview, snapshot?.config.ui.font, snapshot?.config.ui.theme]);

  const previewAppearance = useCallback((preview?: SettingsAppearancePreview): Promise<IpcResult<null>> => {
    setAppearancePreview(preview);
    return appearancePreviewQueue.current(preview);
  }, []);

  useEffect(() => {
    let mounted = true;
    const apply = (next: AppSnapshot): void => {
      if (!mounted) return;
      setSnapshot((current) => current === undefined || next.revision > current.revision ? next : current);
    };
    const snapshots = desktopApi.subscribeSnapshots((event) => apply(event.snapshot));
    const selected = desktopApi.subscribeSelection((id) => {
      setSettingsOpen(false);
      setSettingsFocus(undefined);
      setHistoryOpen(false);
      setSelection((current) => ({ id, request: (current?.request ?? 0) + 1 }));
    });
    const settings = desktopApi.subscribeSettingsRequested(() => setSettingsOpen(true));
    const focusedSettings = desktopApi.subscribeSettingsFocus((section) => {
      setSettingsFocus(section);
      setSettingsOpen(true);
    });
    const load = (): void => {
      setSnapshotRetrying(true);
      void snapshots.ready
        .then(() => desktopApi.getSnapshot())
        .then((result) => {
          if (result.ok) {
            setError(undefined);
            apply(result.value);
          } else {
            setError(result.error.message);
          }
        })
        .finally(() => setSnapshotRetrying(false));
    };
    load();
    void desktopApi.listPersonas().then((result) => {
      if (mounted && result.ok) setPersonas(result.value);
    });
    return () => {
      mounted = false;
      snapshots.dispose();
      selected.dispose();
      settings.dispose();
      focusedSettings.dispose();
    };
  }, []);

  const needsProviderModels = settingsOpen;
  useEffect(() => {
    if (!needsProviderModels) return;
    let active = true;
    if (providerModels.length === 0) setProviderModelsError("AI の一覧を取得しています");
    void desktopApi.providerModels().then((result) => {
      if (!active) return;
      if (result.ok) {
        setProviderModels(result.value);
        setProviderModelsError(undefined);
      } else {
        setProviderModelsError(result.error.message);
      }
    });
    return () => { active = false; };
  }, [needsProviderModels]);

  useEffect(() => {
    if (!settingsOpen) return;
    let active = true;
    setProviderApiKeys(undefined);
    setProviderApiKeysError(undefined);
    void desktopApi.providerApiKeys().then((result) => {
      if (!active) return;
      if (result.ok) {
        setProviderApiKeys(result.value);
        setProviderApiKeysError(undefined);
      } else {
        setProviderApiKeysError(result.error.message);
      }
    });
    return () => { active = false; };
  }, [settingsOpen]);

  useEffect(() => {
    if (snapshot === undefined
      || !tutorialSettingsPresentationPending(snapshot.onboarding, settingsOpen)) return;
    let active = true;
    const frame = window.requestAnimationFrame(() => {
      void desktopApi.tutorialSettingsPresented().then((result) => {
        if (active && !result.ok) setError(result.error.message);
      });
    });
    return () => {
      active = false;
      window.cancelAnimationFrame(frame);
    };
  }, [settingsOpen, snapshot?.onboarding.settingsHighlight]);

  if (snapshot === undefined) {
    if (error !== undefined) {
      return <StartupRecoveryScreen
        error={error}
        retrying={snapshotRetrying}
        onRetry={() => {
          setError(undefined);
          setSnapshotRetrying(true);
          void desktopApi.getSnapshot().then((result) => {
            if (result.ok) {
              setError(undefined);
              setSnapshot(result.value);
            } else setError(result.error.message);
          }).finally(() => setSnapshotRetrying(false));
        }}
        onSettings={() => {
          void desktopApi.settingsRequested();
          setSnapshotRetrying(true);
          void desktopApi.getSnapshot().then((result) => {
            if (result.ok) {
              setError(undefined);
              setSnapshot(result.value);
            } else setError(result.error.message);
          }).finally(() => setSnapshotRetrying(false));
        }}
        onExit={() => { void desktopApi.exit(); }}
      />;
    }
    return <main className="loading-screen"><div className="loading-orb" /><p>起動しています…</p></main>;
  }

  const handle = async <T,>(promise: Promise<IpcResult<T>>): Promise<IpcResult<T>> => {
    const result = await promise;
    if (result.ok) setError(undefined);
    else setError(result.error.message);
    return result;
  };
  const refreshPersonas = async (): Promise<void> => {
    const result = await desktopApi.listPersonas();
    if (result.ok) setPersonas(result.value);
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
  const reloadPersona = async (): Promise<IpcResult<null>> => {
    const result = await handle(desktopApi.reloadPersona());
    await refreshPersonas();
    return result;
  };
  const toggleWatch = async (): Promise<void> => {
    const target = !(requestedWatchIntent ?? snapshot.watchIntentActive);
    const revision = ++watchRequestRevision.current;
    setRequestedWatchIntent(target);
    try {
      await handle(target ? desktopApi.startObserver() : desktopApi.stopObserver());
    } finally {
      if (watchRequestRevision.current === revision) setRequestedWatchIntent(undefined);
    }
  };
  const toggleAudio = async (): Promise<void> => {
    const target = !(requestedAudioIntent ?? snapshot.config.audio.enabled);
    const revision = ++audioRequestRevision.current;
    setRequestedAudioIntent(target);
    try {
      await handle(desktopApi.updateConfig({ audio: { enabled: target } }, undefined, snapshot.configRevision));
    } finally {
      if (audioRequestRevision.current === revision) setRequestedAudioIntent(undefined);
    }
  };
  const requestSettings = (): void => {
    setSettingsFocus(undefined);
    void handle(desktopApi.settingsRequested());
  };
  const resetConversation = async (): Promise<void> => {
    setConversationResetConfirmOpen(false);
    await handle(desktopApi.resetConversation());
  };
  const headerMenuItems = createHeaderMenuItems(
    () => { void handle(desktopApi.openModelPopup()); },
    () => setConversationResetConfirmOpen(true),
    !snapshot.onboarding.tutorialActive,
  );
  const retryFinish = async (): Promise<void> => {
    setFinishRetrying(true);
    try {
      await handle(desktopApi.finishTutorial());
    } finally {
      setFinishRetrying(false);
    }
  };
  const chatResponseFailed = tutorialResponseFailed(snapshot);
  const tutorialNextAvailable = tutorialStepCanBeSkipped(snapshot.onboarding.currentStep);
  const renderedSnapshot = appearancePreview === undefined ? snapshot : {
    ...snapshot,
    config: {
      ...snapshot.config,
      ui: { ...snapshot.config.ui, theme: appearancePreview.theme, font: appearancePreview.font, avatarColor: appearancePreview.avatarColor },
      bubble: { ...snapshot.config.bubble, position: appearancePreview.bubblePosition, display: appearancePreview.bubbleDisplay },
    },
  };

  return <main className="app-shell">
    {snapshot.onboarding.finishPending ? <FinishRecoveryScreen error={error} busy={finishRetrying} onRetry={() => void retryFinish()} /> : snapshot.onboarding.setupRequired ? null : <>
      <Header
      snapshot={renderedSnapshot}
      watchIntentActive={snapshot.watchIntentActive}
      watchChanging={requestedWatchIntent !== undefined}
      onToggleWatch={() => void toggleWatch()}
      audioEnabled={snapshot.config.audio.enabled}
      audioChanging={requestedAudioIntent !== undefined}
      onToggleAudio={() => void toggleAudio()}
      onOpenSettings={requestSettings}
      historyOpen={historyOpen}
      onToggleHistory={() => setHistoryOpen((current) => !current)}
      menuItems={headerMenuItems}
    />
    <NowLine snapshot={renderedSnapshot} onShowDebug={setDebugDetail} onAssertiveness={(value) => { void handle(desktopApi.setAssertiveness(value)); }} />
    <StatusBanner
      snapshot={renderedSnapshot}
      transientError={error}
      onOpenSettings={requestSettings}
      onOpenSystemSettings={() => void handle(desktopApi.openSystemSettings())}
      onRelaunch={() => void handle(desktopApi.relaunch())}
      onOpenSpeechSettings={() => void handle(desktopApi.openSpeechSettings(
        snapshot.speech.microphonePermission !== "granted" ? "microphone" : "recognition",
      ))}
    />
    <ThoughtBubble snapshot={renderedSnapshot} />
    {snapshot.onboarding.tutorialActive ? <div className="tutorial-controls" role="status"><span>{snapshot.onboarding.finishPending ? "終了処理をやり直してください" : chatResponseFailed ? "返事を作れませんでした。もう一度試せます。" : snapshot.onboarding.skipHint ?? "使い方を案内しています"}</span>{snapshot.onboarding.finishPending || snapshot.onboarding.resumePending || !tutorialNextAvailable ? null : chatResponseFailed ? <button type="button" onClick={() => void handle(desktopApi.retryChat())}>やり直す</button> : <button type="button" onClick={() => void handle(desktopApi.tutorialNext())}>次へ</button>}<button type="button" onClick={() => void handle(desktopApi.finishTutorial())}>{snapshot.onboarding.finishPending ? "終了処理を再試行" : "チュートリアルを終了"}</button></div> : null}
      {historyOpen ? <AttachmentHistory snapshot={renderedSnapshot} /> : <Conversation
      snapshot={renderedSnapshot}
      selection={selection}
      onSend={async (message) => (await handle(desktopApi.sendChat(message))).ok}
      onInputActive={(active) => { void desktopApi.setChatInputActive(active); }}
      onRead={() => { void desktopApi.markUnreadRead(); }}
      onCancel={async () => { await handle(desktopApi.cancelChat()); }}
      onRetryAttachment={async () => { await handle(desktopApi.retryChat()); }}
      />}
    </>}
    {!snapshot.onboarding.finishPending && settingsOpen && snapshot.onboarding.tutorialActive && !tutorialSettingsAreAvailable(snapshot.onboarding.currentStep) ? <div className="settings-overlay"><section className="settings-panel tutorial-locked-settings" aria-label="設定"><div className="settings-heading"><div><span>設定</span><h2>CooSenpAI</h2></div><button className="icon-button" type="button" aria-label="設定を閉じる" onClick={() => setSettingsOpen(false)}><CloseIcon /></button></div><p>この項目は、案内の「性格」まで進むと変更できます。</p></section></div> : !snapshot.onboarding.finishPending && settingsOpen ? <SettingsPanel
      snapshot={snapshot}
      personas={personas}
      providerModels={providerModels}
      providerModelsError={providerModelsError}
      providerApiKeys={providerApiKeys}
      providerApiKeysError={providerApiKeysError}
      focusSection={settingsFocus}
      onClose={() => { setSettingsOpen(false); setSettingsFocus(undefined); }}
      onSave={(patch, avatarImage, baseConfigRevision) => handle(desktopApi.updateConfig(patch, avatarImage, baseConfigRevision))}
      onReloadPersona={reloadPersona}
      onGetPersona={(id) => desktopApi.getPersona(id)}
      onSavePersona={async (id, displayName, body) => { const result = await handle(desktopApi.savePersona(id, displayName, body)); await refreshPersonas(); return result; }}
      onDeletePersona={async (id) => { const result = await handle(desktopApi.deletePersona(id)); await refreshPersonas(); return result; }}
      onRestorePersona={(id, version) => handle(desktopApi.restorePersona(id, version))}
      onRestartTutorial={() => handle(desktopApi.restartTutorial())}
      onRestartSetup={() => handle(desktopApi.restartSetup())}
      onResetConversation={() => handle(desktopApi.resetConversation())}
      onOpenSystemSettings={() => void handle(desktopApi.openSystemSettings())}
      onOpenSpeechSettings={(kind) => void handle(desktopApi.openSpeechSettings(kind))}
      onRelaunch={() => void handle(desktopApi.relaunch())}
      onAppearancePreview={previewAppearance}
      onSaveProviderApiKey={saveProviderApiKey}
      onDeleteProviderApiKey={deleteProviderApiKey}
    /> : null}
    {debugDetail === undefined ? null : <DebugDrawer detail={debugDetail} close={() => setDebugDetail(undefined)} />}
    {conversationResetConfirmOpen ? <ConfirmationDialog id="conversation-reset" title="会話をリセットしますか？" description="表示を白紙にして、新しい会話を始めます。履歴は残ります。" cancelLabel="キャンセル" confirmLabel="リセットする" onCancel={() => setConversationResetConfirmOpen(false)} onConfirm={() => { void resetConversation(); }} /> : null}
  </main>;
}
