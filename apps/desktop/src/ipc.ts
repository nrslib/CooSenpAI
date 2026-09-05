import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AppSnapshot, BubbleSnapshot, CapturePopupSnapshot, CompanionModelCatalog, ConfigPatch, CooSenpaiConfig, IpcResult, MemoryCatalog, PersonaDocument, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName, RunningApplication, SettingsAppearancePreviewPayload, SnapshotEvent, SpeechPopupSnapshot } from "./types.js";

export const ipcChannels = {
  snapshotUpdated: "coosenpai:snapshot:updated",
  conversationSelected: "coosenpai:conversation:selected",
  settingsRequested: "coosenpai:settings:requested",
  settingsFocus: "coosenpai:settings:focus",
  bubbleShow: "coosenpai:bubble:show",
  capturePopupReady: "coosenpai:capture-popup:ready",
  speechComposerFinal: "coosenpai:speech:composer-final",
} as const;

async function call<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args);
  } catch (error) {
    return { ok: false, error: { message: ipcTransportError(error) } };
  }
}

const speechIpcFailureMessage = "音声入力に失敗しました";

async function callSpeech<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args);
  } catch (error) {
    console.warn("音声入力 IPC エラー", error);
    return { ok: false, error: { message: speechIpcFailureMessage } };
  }
}

export function ipcTransportError(error: unknown): string {
  if (error instanceof Error && error.message.trim() !== "") return error.message;
  if (typeof error === "string" && error.trim() !== "") return error;
  return "アプリとの通信に失敗しました";
}

function subscribe<T>(channel: string, listener: (payload: T) => void): { readonly dispose: () => void; readonly ready: Promise<void> } {
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  const ready = listen<T>(channel, (event) => listener(event.payload)).then((value) => {
    if (disposed) value();
    else unlisten = value;
  });
  return { dispose: () => { disposed = true; unlisten?.(); }, ready };
}

export const desktopApi = {
  getSnapshot: (): Promise<IpcResult<AppSnapshot>> => call("snapshot_get"),
  startObserver: (): Promise<IpcResult<AppSnapshot>> => call("watch_start"),
  stopObserver: (): Promise<IpcResult<AppSnapshot>> => call("watch_stop"),
  listRunningApps: (): Promise<IpcResult<readonly RunningApplication[]>> => call("running_apps_list"),
  addWatchTarget: (bundleId: string): Promise<IpcResult<CooSenpaiConfig>> => call("watch_target_add", { payload: { bundleId } }),
  removeWatchTarget: (bundleId: string): Promise<IpcResult<CooSenpaiConfig>> => call("watch_target_remove", { payload: { bundleId } }),
  setWatchTargetEnabled: (bundleId: string, enabled: boolean): Promise<IpcResult<CooSenpaiConfig>> => call("watch_target_set_enabled", { payload: { bundleId, enabled } }),
  sendChat: (message: string): Promise<IpcResult<string>> => call("chat_send", { payload: { message } }),
  cancelChat: (): Promise<IpcResult<string>> => call("chat_cancel"),
  retryChat: (): Promise<IpcResult<string>> => call("chat_retry"),
  readAttachment: (path: string): Promise<IpcResult<readonly number[]>> => call("attachment_read", { payload: { path } }),
  getConfig: (): Promise<IpcResult<CooSenpaiConfig>> => call("config_get"),
  getPersistedConfig: (): Promise<IpcResult<CooSenpaiConfig>> => call("config_get_persisted"),
  updateConfig: (patch: ConfigPatch, avatarImage?: readonly number[], baseConfigRevision?: number): Promise<IpcResult<CooSenpaiConfig>> => call(
    "config_update",
    {
      patch,
      ...(avatarImage === undefined ? {} : { avatarImage }),
      ...(baseConfigRevision === undefined ? {} : { baseConfigRevision }),
    },
  ),
  openModelPopup: (): Promise<IpcResult<null>> => call("model_popup_open"),
  previewSettingsAppearance: (preview?: SettingsAppearancePreviewPayload): Promise<IpcResult<null>> => call("settings_appearance_preview", { payload: preview ?? null }),
  setAssertiveness: (value: "low" | "normal" | "high"): Promise<IpcResult<CooSenpaiConfig>> => call("companion_assertiveness_set", { payload: { value } }),
  listPersonas: (): Promise<IpcResult<readonly PersonaOption[]>> => call("persona_list"),
  providerModels: (): Promise<IpcResult<readonly ProviderModelOptions[]>> => call("provider_models"),
  providerApiKeys: (): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_keys_get"),
  setProviderApiKey: (provider: ProviderName, apiKey: string): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_key_set", { payload: { provider, apiKey } }),
  deleteProviderApiKey: (provider: ProviderName): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_key_delete", { payload: { provider } }),
  selectPersona: (persona: string): Promise<IpcResult<CooSenpaiConfig>> => call("persona_select", { payload: { persona } }),
  reloadPersona: (): Promise<IpcResult<null>> => call("persona_reload"),
  getPersona: (id: string): Promise<IpcResult<PersonaDocument>> => call("persona_get", { payload: { id } }),
  savePersona: (id: string, displayName: string, body: string): Promise<IpcResult<CooSenpaiConfig>> => call("persona_save", { payload: { id, displayName, body } }),
  deletePersona: (id: string): Promise<IpcResult<CooSenpaiConfig>> => call("persona_delete", { payload: { id } }),
  restorePersona: (id: string, version: string): Promise<IpcResult<null>> => call("persona_restore", { payload: { id, version } }),
  tutorialNext: (): Promise<IpcResult<AppSnapshot>> => call("tutorial_next"),
  tutorialSettingsPresented: (): Promise<IpcResult<AppSnapshot>> => call("tutorial_settings_presented"),
  finishTutorial: (): Promise<IpcResult<AppSnapshot>> => call("tutorial_finish"),
  restartTutorial: (): Promise<IpcResult<AppSnapshot>> => call("tutorial_restart"),
  restartSetup: (): Promise<IpcResult<AppSnapshot>> => call("setup_restart"),
  resetConversation: (): Promise<IpcResult<AppSnapshot>> => call("conversation_reset"),
  listMemory: (): Promise<IpcResult<MemoryCatalog>> => call("memory_list"),
  confirmMemory: (candidateId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_confirm", { payload: { candidateId, confirmationId } }),
  rejectMemory: (candidateId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_reject", { payload: { candidateId } }),
  confirmMemoryUpdate: (updateId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_confirm_update", { payload: { updateId, confirmationId } }),
  rejectMemoryUpdate: (updateId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_reject_update", { payload: { updateId } }),
  deleteMemory: (factId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_delete", { payload: { factId, confirmationId } }),
  consolidateMemory: (period: string): Promise<IpcResult<MemoryCatalog>> => call("memory_consolidate", { payload: { period } }),
  openSystemSettings: (): Promise<IpcResult<null>> => call("panel_open_system_settings"),
  relaunch: (): Promise<IpcResult<null>> => call("app_relaunch"),
  exit: (): Promise<IpcResult<null>> => call("app_exit"),
  setChatInputActive: (active: boolean): Promise<IpcResult<null>> => call("chat_input_state", { payload: { active } }),
  markUnreadRead: (): Promise<IpcResult<null>> => call("unread_read"),
  adviceSelected: (id: string): Promise<IpcResult<null>> => call("advice_selected", { payload: { id } }),
  settingsRequested: (): Promise<IpcResult<null>> => call("settings_requested"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribe(ipcChannels.snapshotUpdated, listener),
  subscribeSelection: (listener: (id: string) => void) => subscribe(ipcChannels.conversationSelected, listener),
  subscribeSettingsRequested: (listener: () => void) => subscribe<void>(ipcChannels.settingsRequested, listener),
  subscribeSettingsFocus: (listener: (section: "watch") => void) => subscribe<"watch">(ipcChannels.settingsFocus, listener),
  startSpeech: (): Promise<IpcResult<null>> => call("speech_start", { payload: { source: "composer" } }),
  finishSpeech: (): Promise<IpcResult<null>> => call("speech_finish"),
  cancelSpeech: (): Promise<IpcResult<null>> => call("speech_cancel"),
  openSpeechSettings: (kind: "microphone" | "recognition"): Promise<IpcResult<null>> => call("speech_open_system_settings", { payload: { kind } }),
  subscribeSpeechComposerFinal: (listener: (text: string) => void) => subscribe(ipcChannels.speechComposerFinal, listener),
};

export const speechPopupApi = {
  getSnapshot: (): Promise<IpcResult<SpeechPopupSnapshot>> => callSpeech("speech_popup_snapshot"),
  send: (text: string): Promise<IpcResult<string>> => callSpeech("speech_popup_send", { payload: { text } }),
  cancel: (): Promise<IpcResult<null>> => callSpeech("speech_popup_cancel"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribe(ipcChannels.snapshotUpdated, listener),
};

export const modelPopupApi = {
  getSnapshot: (): Promise<IpcResult<AppSnapshot>> => call("model_popup_snapshot"),
  updateConfig: (patch: ConfigPatch): Promise<IpcResult<CooSenpaiConfig>> => call("model_popup_config_update", { patch }),
  companionModelCatalog: (): Promise<IpcResult<CompanionModelCatalog>> => call("model_popup_companion_model_catalog"),
  reloadOpencodeModels: (): Promise<IpcResult<CompanionModelCatalog>> => call("model_popup_opencode_models_reload"),
  close: (): Promise<IpcResult<null>> => call("model_popup_close"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribe(ipcChannels.snapshotUpdated, listener),
};

export const bubbleApi = {
  getSnapshot: (): Promise<IpcResult<BubbleSnapshot>> => call("bubble_snapshot"),
  rendererReady: (attempt: number): Promise<IpcResult<BubbleSnapshot>> => call("bubble_renderer_ready", { payload: { attempt } }),
  subscribeShow: (listener: (snapshot: BubbleSnapshot) => void) => subscribe(ipcChannels.bubbleShow, listener),
  ack: (generation: number): Promise<IpcResult<null>> => call("bubble_ack", { payload: { generation } }),
  dismiss: (id: string): Promise<IpcResult<null>> => call("bubble_dismiss", { payload: { id } }),
  fastForward: (id?: string): Promise<IpcResult<boolean>> => call("tutorial_sequence_fast_forward", { payload: id === undefined ? null : { id } }),
  click: (id: string): Promise<IpcResult<null>> => call("bubble_click", { payload: { id } }),
  hover: (id: string, hovering: boolean): Promise<IpcResult<null>> => call("bubble_hover", { payload: { id, hovering } }),
  focus: (): Promise<IpcResult<null>> => call("bubble_focus"),
  passThrough: (): Promise<IpcResult<null>> => call("bubble_passthrough"),
  resize: (height: number): Promise<IpcResult<null>> => call("bubble_resize", { payload: { height } }),
  interact: (id: string, action: string, value?: string): Promise<IpcResult<null>> => call("bubble_interact", { payload: { id, action, value } }),
};

export const capturePopupApi = {
  getSnapshot: (): Promise<IpcResult<CapturePopupSnapshot>> => call("capture_popup_snapshot"),
  send: (captureId: string, message: string): Promise<IpcResult<string>> => call("capture_popup_send", { payload: { captureId, message } }),
  cancel: (): Promise<IpcResult<null>> => call("capture_popup_cancel"),
  openAccessibilitySettings: (): Promise<IpcResult<null>> => call("capture_popup_open_accessibility_settings"),
  subscribeReady: (listener: (captureId: string) => void) => subscribe<string>(ipcChannels.capturePopupReady, listener),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribe(ipcChannels.snapshotUpdated, listener),
};
