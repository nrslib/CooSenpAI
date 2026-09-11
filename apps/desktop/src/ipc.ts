import type { PanelCommand, PanelEvent, PanelKind, PanelOutput } from "./usePanelPresenter.js";
import type { UpdateSnapshot } from "./app-update.js";
import type { VoiceOutputSnapshot, VoiceOutputVoice } from "./voice-output.js";
import type { AvatarSnapshot } from "./avatar/state.js";
import { t, type Locale } from "./i18n/index.js";
import type { WorkSnapshot } from "./work.js";

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { AppSnapshot, BubbleSnapshot, CapturePopupSnapshot, CompanionModelCatalog, ConfigPatch, CooSenpaiConfig, DataFlowLog, IpcResult, LicenseDocument, MemoryCatalog, PersonaDocument, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName, RunningApplication, SettingsAppearancePreviewPayload, SnapshotEvent, SpeechPopupSnapshot } from "./types.js";

// 各 WebView は独立した JS コンテキストなので、最後に受け取った snapshot の言語を
// IPC の transport 例外にも使う。最初の snapshot 前は config の既定値 ja とする。
let ipcLocale: Locale = "ja";

export function setIpcLocale(locale: Locale): void {
  ipcLocale = locale;
}

export const ipcChannels = {
  snapshotUpdated: "coosenpai:snapshot:updated",
  conversationSelected: "coosenpai:conversation:selected",
  settingsRequested: "coosenpai:settings:requested",
  settingsFocus: "coosenpai:settings:focus",
  composerFocus: "coosenpai:composer:focus",
  bubbleShow: "coosenpai:bubble:show",
} as const;

async function call<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args);
  } catch (error) {
    return { ok: false, error: { message: ipcTransportError(error, ipcLocale) } };
  }
}

async function callSpeech<T>(command: string, args?: Record<string, unknown>): Promise<IpcResult<T>> {
  try {
    return await invoke<IpcResult<T>>(command, args);
  } catch (error) {
    console.warn("Speech input IPC error", error);
    return { ok: false, error: { message: t(ipcLocale, "common.speechIpcFailure") } };
  }
}

export function ipcTransportError(error: unknown, locale: Locale = ipcLocale): string {
  if (error instanceof Error && error.message.trim() !== "") return error.message;
  if (typeof error === "string" && error.trim() !== "") return error;
  return t(locale, "common.ipcFailure");
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

// 会話本文を含むイベントは backend が対象 WebView 内だけに配信する。
function subscribeLocal<T>(channel: string, listener: (payload: T) => void): { readonly dispose: () => void; readonly ready: Promise<void> } {
  const receive = (event: Event): void => listener((event as CustomEvent<T>).detail);
  window.addEventListener(channel, receive);
  return { dispose: () => window.removeEventListener(channel, receive), ready: Promise.resolve() };
}

export const desktopApi = {
  panelEvent: <S, C extends PanelCommand>(payload: { readonly session: string; readonly kind: PanelKind; readonly event: PanelEvent }): Promise<IpcResult<PanelOutput<S, C>>> => call("ui_panel_event", { payload }),
  appInput: (payload: import("./app-view.js").AppInput): Promise<IpcResult<null>> => call("app_view_input", { payload }),
  appSelectPersona: (persona: string): Promise<IpcResult<CooSenpaiConfig>> => call("app_select_persona", { persona }),
  subscribeAppView: (listener: (view: import("./app-view.js").AppView) => void) => subscribeLocal("coosenpai:app:view", listener),
  subscribeStatusView: (listener: (view: import("./app-view.js").StatusView) => void) => subscribeLocal("coosenpai:status:view", listener),
  workApprovalInput: (payload: import("./work-approval-view.js").WorkApprovalInput): Promise<IpcResult<null>> => call("work_approval_input", { payload }),
  subscribeWorkApprovalView: (listener: (view: import("./work-approval-view.js").WorkApprovalView) => void) => subscribeLocal("coosenpai:work-approval:view", listener),
  composerInput: (payload: import("./composer-view.js").ComposerInput): Promise<IpcResult<null>> => call("composer_input", { payload }),
  conversationInput: (payload: import("./composer-view.js").ConversationInput): Promise<IpcResult<null>> => call("conversation_input", { payload }),
  subscribeComposerView: (listener: (view: import("./composer-view.js").ComposerView) => void) => subscribeLocal("coosenpai:composer:view", listener),
  subscribeConversationView: (listener: (view: import("./composer-view.js").ConversationView) => void) => subscribeLocal("coosenpai:conversation:view", listener),
  ready: (): Promise<IpcResult<null>> => call("ui_view_mounted"),
  closeSettings: (): Promise<IpcResult<null>> => call("settings_close"),
  subscribeSettingsClosed: (listener: () => void) => subscribe("coosenpai:settings:closed", listener),
  subscribePersonas: (listener: (personas: PersonaOption[]) => void) => subscribeLocal("coosenpai:personas:load", listener),
  subscribeSettingsResources: (listener: (resources: { models: IpcResult<ProviderModelOptions[]>; keys: IpcResult<ProviderApiKeyStatus> }) => void) => subscribeLocal("coosenpai:settings:resources", listener),
  openVoiceOutputLink: (kind: "download" | "terms"): Promise<IpcResult<null>> => call("voice_output_open_link", { payload: { kind } }),
  getVoiceOutputVoices: (): Promise<IpcResult<readonly VoiceOutputVoice[]>> => call("voice_output_voices"),
  workStatus: (): Promise<IpcResult<WorkSnapshot>> => call("work_status"),
  stopWork: (id: string): Promise<IpcResult<WorkSnapshot>> => call("work_stop", { payload: { id } }),
  approveWork: (id: string, decision: "allow" | "deny"): Promise<IpcResult<WorkSnapshot>> => call("work_approve", { payload: { id, decision } }),
  getVoiceOutputStatus: (): Promise<IpcResult<VoiceOutputSnapshot>> => call("voice_output_status"),
  testVoiceOutput: (): Promise<IpcResult<VoiceOutputSnapshot>> => call("voice_output_test"),
  stopVoiceOutput: (): Promise<IpcResult<VoiceOutputSnapshot>> => call("voice_output_stop"),
  subscribeVoiceOutput: (listener: (snapshot: VoiceOutputSnapshot) => void) => subscribe("coosenpai:voice-output:changed", listener),
  getAppUpdateStatus: (): Promise<IpcResult<UpdateSnapshot>> => call("app_update_status"),
  checkAppUpdate: (): Promise<IpcResult<UpdateSnapshot>> => call("app_update_check"),
  installAppUpdate: (): Promise<IpcResult<UpdateSnapshot>> => call("app_update_install"),
  subscribeAppUpdates: (listener: (snapshot: UpdateSnapshot) => void) => subscribe("coosenpai:update:changed", listener),
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
  updateConfig: (patch: ConfigPatch, avatarImage: readonly number[] | undefined, baseConfigRevision: number): Promise<IpcResult<CooSenpaiConfig>> => call(
    "config_update",
    {
      patch,
      ...(avatarImage === undefined ? {} : { avatarImage }),
      ...(baseConfigRevision === undefined ? {} : { baseConfigRevision }),
    },
  ),
  openModelPopup: (): Promise<IpcResult<null>> => call("model_popup_open"),
  openDetails: (): Promise<IpcResult<null>> => call("details_open"),
  previewSettingsAppearance: (preview?: SettingsAppearancePreviewPayload): Promise<IpcResult<null>> => call("settings_appearance_preview", { payload: preview ?? null }),
  setAssertiveness: (value: "low" | "normal" | "high"): Promise<IpcResult<CooSenpaiConfig>> => call("companion_assertiveness_set", { payload: { value } }),
  listPersonas: (): Promise<IpcResult<readonly PersonaOption[]>> => call("persona_list"),
  providerModels: (): Promise<IpcResult<readonly ProviderModelOptions[]>> => call("provider_models"),
  providerApiKeys: (): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_keys_get"),
  setProviderApiKey: (provider: ProviderName, apiKey: string): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_key_set", { payload: { provider, apiKey } }),
  deleteProviderApiKey: (provider: ProviderName): Promise<IpcResult<ProviderApiKeyStatus>> => call("provider_api_key_delete", { payload: { provider } }),
  selectPersona: (persona: string): Promise<IpcResult<CooSenpaiConfig>> => call("persona_select", { payload: { persona } }),
  selectPersonaForSetup: (persona: string): Promise<IpcResult<CooSenpaiConfig>> => call("persona_select_setup", { payload: { persona } }),
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
  resetCompanionEmotions: (): Promise<IpcResult<AppSnapshot>> => call("companion_emotions_reset"),
  listMemory: (): Promise<IpcResult<MemoryCatalog>> => call("memory_list"),
  confirmMemory: (candidateId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_confirm", { payload: { candidateId, confirmationId } }),
  rejectMemory: (candidateId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_reject", { payload: { candidateId } }),
  confirmMemoryUpdate: (updateId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_confirm_update", { payload: { updateId, confirmationId } }),
  rejectMemoryUpdate: (updateId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_reject_update", { payload: { updateId } }),
  deleteMemory: (factId: string, confirmationId: string): Promise<IpcResult<MemoryCatalog>> => call("memory_delete", { payload: { factId, confirmationId } }),
  consolidateMemory: (period: string): Promise<IpcResult<MemoryCatalog>> => call("memory_consolidate", { payload: { period } }),
  openSystemSettings: (): Promise<IpcResult<null>> => call("panel_open_system_settings"),
  openLicenseDocument: (document: LicenseDocument): Promise<IpcResult<null>> => call("license_document_open", { payload: { document } }),
  relaunch: (): Promise<IpcResult<null>> => call("app_relaunch"),
  exit: (): Promise<IpcResult<null>> => call("app_exit"),
  setChatInputActive: (active: boolean): Promise<IpcResult<null>> => call("chat_input_state", { payload: { active } }),
  markUnreadRead: (): Promise<IpcResult<null>> => call("unread_read"),
  adviceSelected: (id: string): Promise<IpcResult<null>> => call("advice_selected", { payload: { id } }),
  settingsRequested: (): Promise<IpcResult<null>> => call("settings_requested"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribeLocal(ipcChannels.snapshotUpdated, listener),
  subscribeSelection: (listener: (id: string) => void) => subscribe(ipcChannels.conversationSelected, listener),
  subscribeSettingsRequested: (listener: () => void) => subscribe<void>(ipcChannels.settingsRequested, listener),
  subscribeSettingsFocus: (listener: (section: "watch") => void) => subscribe<"watch">(ipcChannels.settingsFocus, listener),
  subscribeComposerFocus: (listener: () => void) => subscribe<void>(ipcChannels.composerFocus, listener),
  startSpeech: (): Promise<IpcResult<null>> => call("speech_start", { payload: { source: "composer" } }),
  finishSpeech: (): Promise<IpcResult<null>> => call("speech_finish"),
  cancelSpeech: (): Promise<IpcResult<null>> => call("speech_cancel"),
  openSpeechSettings: (kind: "microphone" | "recognition"): Promise<IpcResult<null>> => call("speech_open_system_settings", { payload: { kind } }),
};

export const motionApi = {
  input: (payload: import("./vrm/motion-view.js").MotionInput): Promise<IpcResult<null>> => call("motion_settings_input", { payload }),
  subscribeView: (listener: (view: import("./vrm/motion-view.js").MotionView) => void) => subscribeLocal("coosenpai:motion-settings:view", listener),
  subscribeStorage: (listener: (command: import("./vrm/motion-view.js").MotionStorageCommand) => void) => subscribeLocal("coosenpai:motion-settings:storage", listener),
};

export const avatarApi = {
  sceneInput: (payload: import("./avatar/scene-view.js").AvatarSceneInput): Promise<IpcResult<null>> => call("avatar_scene_input", { payload }),
  subscribeSceneView: (listener: (view: import("./avatar/scene-view.js").AvatarSceneView) => void) => subscribeLocal("coosenpai:avatar-scene:view", listener),
  ready: (): Promise<IpcResult<null>> => call("ui_view_mounted"),
  getState: (): Promise<IpcResult<AvatarSnapshot>> => call("avatar_state_get"),
  show: (): Promise<IpcResult<null>> => call("avatar_show"),
  hide: (): Promise<IpcResult<null>> => call("avatar_hide"),
  toggle: (): Promise<IpcResult<null>> => call("avatar_toggle"),
  modelChanged: (): Promise<IpcResult<AvatarSnapshot>> => call("avatar_model_changed"),
  subscribe: (listener: (snapshot: AvatarSnapshot) => void) => subscribe("coosenpai:avatar:changed", listener),
};

export const speechPopupApi = {
  getSnapshot: (): Promise<IpcResult<SpeechPopupSnapshot>> => callSpeech("speech_popup_snapshot"),
  ready: (): Promise<IpcResult<null>> => callSpeech("ui_view_mounted"),
  send: (generation: number): Promise<IpcResult<string>> => callSpeech("speech_popup_send", { payload: { generation } }),
  edit: (generation: number, editRevision: number, text: string): Promise<IpcResult<null>> => callSpeech("speech_popup_edit", { payload: { generation, editRevision, text } }),
  key: (generation: number, input: { key: string; metaKey: boolean; shiftKey: boolean; composing: boolean; keyCode: number }): Promise<IpcResult<null>> => callSpeech("speech_popup_key", { payload: { generation, input } }),
  subscribeLoad: (listener: (snapshot: SpeechPopupSnapshot) => void) => subscribeLocal("coosenpai:speech-popup:load", listener),
  subscribeInput: (listener: (input: { generation: number; editRevision: number; text: string; sending: boolean; canSend: boolean; error?: string }) => void) => subscribeLocal("coosenpai:speech-popup:input", listener),
  subscribeFocus: (listener: () => void) => subscribeLocal("coosenpai:speech-popup:focus", listener),
  cancel: (generation: number): Promise<IpcResult<null>> => callSpeech("speech_popup_cancel", { payload: { generation } }),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribeLocal(ipcChannels.snapshotUpdated, listener),
};

export const modelPopupApi = {
  input: (payload: import("./model-popup/view.js").ModelPickerInput): Promise<IpcResult<null>> => call("model_picker_input", { payload }),
  subscribeView: (listener: (view: import("./model-popup/view.js").ModelPickerView) => void) => subscribeLocal("coosenpai:model-picker:view", listener),
  ready: (): Promise<IpcResult<null>> => call("ui_view_mounted"),
  subscribeCatalog: (listener: (catalog: CompanionModelCatalog) => void) => subscribeLocal("coosenpai:model-catalog:load", listener),
  getSnapshot: (): Promise<IpcResult<AppSnapshot>> => call("model_popup_snapshot"),
  updateConfig: (patch: ConfigPatch): Promise<IpcResult<CooSenpaiConfig>> => call("model_popup_config_update", { patch }),
  companionModelCatalog: (): Promise<IpcResult<CompanionModelCatalog>> => call("model_popup_companion_model_catalog"),
  reloadOpencodeModels: (): Promise<IpcResult<CompanionModelCatalog>> => call("model_popup_opencode_models_reload"),
  close: (): Promise<IpcResult<null>> => call("model_popup_close"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribeLocal(ipcChannels.snapshotUpdated, listener),
};

export const detailsApi = {
  subscribeDataFlow: (listener: (result: IpcResult<DataFlowLog>) => void) => subscribeLocal("coosenpai:dataflow:load", listener),
  ready: (): Promise<IpcResult<null>> => call("ui_view_mounted"),
  getSnapshot: (): Promise<IpcResult<AppSnapshot>> => call("details_snapshot"),
  getDataFlowLog: (): Promise<IpcResult<DataFlowLog>> => call("details_dataflow_log"),
  openDataFlowPath: (path: string): Promise<IpcResult<null>> => call("details_dataflow_open_path", { payload: { path } }),
  resetCompanionEmotions: (): Promise<IpcResult<AppSnapshot>> => call("companion_emotions_reset"),
  selectConversationGeneration: (generation: number): Promise<IpcResult<AppSnapshot>> => call("conversation_select", { payload: { generation } }),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribeLocal(ipcChannels.snapshotUpdated, listener),
};

export const bubbleApi = {
  input: (payload: import("./bubble/view.js").BubbleViewInput): Promise<IpcResult<null>> => call("bubble_view_input", { payload }),
  subscribeControls: (listener: (view: import("./bubble/view.js").BubbleControlsView) => void) => subscribeLocal("coosenpai:bubble:controls", listener),
  subscribeClear: (listener: () => void) => subscribeLocal("coosenpai:bubble:clear", listener),
  subscribeTyping: (listener: (typing: { id: string; revealed: number | null }) => void) => subscribeLocal("coosenpai:bubble:typing", listener),
  getSnapshot: (): Promise<IpcResult<BubbleSnapshot>> => call("bubble_snapshot"),
  rendererReady: (attempt: number): Promise<IpcResult<null>> => call("bubble_renderer_ready", { payload: { attempt } }),
  subscribeShow: (listener: (snapshot: BubbleSnapshot) => void) => subscribeLocal(ipcChannels.bubbleShow, listener),
  ack: (generation: number): Promise<IpcResult<null>> => call("bubble_ack", { payload: { generation } }),
  dismiss: (id: string): Promise<IpcResult<null>> => call("bubble_dismiss", { payload: { id } }),
  fastForward: (id?: string): Promise<IpcResult<boolean>> => call("bubble_fast_forward", { payload: id === undefined ? null : { id } }),
  navigate: (direction: "older" | "newer" | "latest"): Promise<IpcResult<boolean>> => call("bubble_navigate", { payload: { direction } }),
  bodyClick: (id: string): Promise<IpcResult<null>> => call("bubble_click", { payload: { id, body: true } }),
  click: (id: string): Promise<IpcResult<null>> => call("bubble_click", { payload: { id } }),
  hover: (id: string, hovering: boolean): Promise<IpcResult<null>> => call("bubble_hover", { payload: { id, hovering } }),
  focus: (): Promise<IpcResult<null>> => call("bubble_focus"),
  passThrough: (): Promise<IpcResult<null>> => call("bubble_passthrough"),
  resize: (height: number): Promise<IpcResult<null>> => call("bubble_resize", { payload: { height } }),
  interact: (id: string, action: string, value?: string): Promise<IpcResult<null>> => call("bubble_interact", { payload: { id, action, value } }),
};

export const capturePopupApi = {
  retry: (generation: number): Promise<IpcResult<null>> => call("capture_popup_retry", { payload: { generation } }),
  ready: (): Promise<IpcResult<null>> => call("ui_view_mounted"),
  edit: (generation: number, editRevision: number, message: string): Promise<IpcResult<null>> => call("capture_popup_edit", { payload: { generation, editRevision, message } }),
  key: (generation: number, input: { readonly key: string; readonly metaKey: boolean; readonly shiftKey: boolean; readonly composing: boolean; readonly keyCode: number }): Promise<IpcResult<null>> => call("capture_popup_key", { payload: { generation, input } }),
  action: (generation: number, index: number): Promise<IpcResult<null>> => call("capture_popup_action", { payload: { generation, index } }),
  subscribeSession: (listener: (generation: number) => void) => subscribeLocal<number>("coosenpai:capture-popup:session", listener),
  subscribeLoad: (listener: (snapshot: CapturePopupSnapshot) => void) => subscribeLocal<CapturePopupSnapshot>("coosenpai:capture-popup:load", listener),
  subscribeInput: (listener: (input: { readonly generation: number; readonly editRevision: number; readonly message: string; readonly sending: boolean; readonly canSend: boolean; readonly previewState: "loading" | "ready" | "failed"; readonly error: string | null }) => void) => subscribeLocal("coosenpai:capture-popup:input", listener),
  subscribeFocus: (listener: () => void) => subscribeLocal<void>("coosenpai:capture-popup:focus", listener),
  getSnapshot: (): Promise<IpcResult<CapturePopupSnapshot>> => call("capture_popup_snapshot"),
  send: (captureId: string): Promise<IpcResult<string>> => call("capture_popup_send", { payload: { captureId } }),
  cancel: (generation: number, source: "esc" | "closeButton"): Promise<IpcResult<null>> => call("capture_popup_cancel", { payload: { generation, source } }),
  openAccessibilitySettings: (): Promise<IpcResult<null>> => call("capture_popup_open_accessibility_settings"),
  subscribeSnapshots: (listener: (event: SnapshotEvent) => void) => subscribeLocal(ipcChannels.snapshotUpdated, listener),
};
