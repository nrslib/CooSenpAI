import { useEffect, useRef, useState, type ChangeEvent, type ComponentProps, type ReactElement } from "react";

import type { AppSnapshot, ConfigIssue, ConfigPatch, CooSenpaiConfig, IpcResult, PersonaDocument, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "./types.js";
import { ConfirmationDialog } from "./components/ConfirmationDialog.js";
import { GeneralSettings } from "./components/GeneralSettings.js";
import { HearingSettings } from "./components/HearingSettings.js";
import { NotificationSettings } from "./components/NotificationSettings.js";
import { ProviderSettings } from "./components/ProviderSettings.js";
import { SettingsTabs } from "./components/SettingsTabs.js";
import { SettingsDiscardDialog } from "./components/SettingsDiscardDialog.js";
import { SetupSettings } from "./components/SetupSettings.js";
import { ShortcutsSettings } from "./components/ShortcutsSettings.js";
import { SpeechSettings } from "./components/SpeechSettings.js";
import { TutorialPersonaSettings } from "./components/TutorialPersonaSettings.js";
import { VisionSettings } from "./components/VisionSettings.js";
import { CloseIcon } from "./components/LineIcons.js";
import { PersonaEditor } from "./components/PersonaEditor.js";
import { CONFIG_REVISION_CONFLICT_MESSAGE, isConfigRevisionConflict, modelAfterProviderChange, settingsIssueHeading, settingsIssueTarget, unavailableProviderMessage } from "./settings-model.js";
import { createSettingsEscapeListener, focusFirstSettingsControl, shouldCloseSettingsDraft } from "./settings-keyboard.js";
import { isTutorialPersonaSettings, settingsCategoryForFocus, settingsCategoryForIssue, type SettingsCategory } from "./settings-categories.js";
import { appearancePreview, defaultTuningForm, hasDraftChanges, toForm, toPatch, type FormState, type SettingsAppearancePreview } from "./settings-form.js";
import { changedConfigPaths, changedConfigPatch, mergeConfigPatch } from "./settings-save.js";

export type { SettingsAppearancePreview } from "./settings-form.js";
export { resetShortcutToDefault, shortcutFromKeyboardEvent } from "./components/ShortcutsSettings.js";

const AVATAR_CONFIG_PATH = "state/avatar.png";
const MAX_AVATAR_UPLOAD_BYTES = 20 * 1024 * 1024;

interface Props {
  readonly snapshot: AppSnapshot;
  readonly personas: readonly PersonaOption[];
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerModelsError?: string;
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly focusSection?: "watch";
  readonly onClose: () => void;
  readonly onSave: (patch: ConfigPatch, avatarImage?: readonly number[], baseConfigRevision?: number) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onReloadConfig: () => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onReloadPersona: () => Promise<IpcResult<null>>;
  readonly onGetPersona: (id: string) => Promise<IpcResult<PersonaDocument>>;
  readonly onSavePersona: (id: string, displayName: string, body: string) => Promise<IpcResult<unknown>>;
  readonly onDeletePersona: (id: string) => Promise<IpcResult<unknown>>;
  readonly onRestorePersona: (id: string, version: string) => Promise<IpcResult<unknown>>;
  readonly onRestartTutorial: () => Promise<IpcResult<unknown>>;
  readonly onRestartSetup: () => Promise<IpcResult<unknown>>;
  readonly onResetConversation: () => Promise<IpcResult<unknown>>;
  readonly onOpenSystemSettings: () => void;
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
  readonly onRelaunch: () => void;
  readonly onAppearancePreview: (preview?: SettingsAppearancePreview) => Promise<IpcResult<null>>;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export { SettingsDiscardDialog } from "./components/SettingsDiscardDialog.js";

type SettingsConfirmation = "tuning" | "conversation-reset" | undefined;

export function SettingsPanel({ snapshot, personas, providerModels, providerModelsError, providerApiKeys, providerApiKeysError, focusSection, onClose, onSave, onReloadConfig, onReloadPersona, onGetPersona, onSavePersona, onDeletePersona, onRestorePersona, onRestartTutorial, onRestartSetup, onResetConversation, onOpenSystemSettings, onOpenSpeechSettings, onRelaunch, onAppearancePreview, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  const [form, setForm] = useState(() => toForm(snapshot.config, snapshot.avatarImageLoadFailed));
  const [issues, setIssues] = useState<readonly ConfigIssue[]>(snapshot.lastError?.kind === "config" ? snapshot.lastError.issues ?? [] : []);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [externalChanges, setExternalChanges] = useState<readonly string[]>();
  const [dirty, setDirty] = useState(false);
  const [discardConfirmOpen, setDiscardConfirmOpen] = useState(false);
  const [confirmation, setConfirmation] = useState<SettingsConfirmation>();
  const [recordingShortcut, setRecordingShortcut] = useState<string>();
  const [personaDocument, setPersonaDocument] = useState<PersonaDocument>();
  const [activeCategory, setActiveCategory] = useState<SettingsCategory>(() => settingsCategoryForFocus(focusSection, snapshot.onboarding.settingsHighlight) ?? "general");
  const formRef = useRef(form);
  const savedConfigRef = useRef(snapshot.config);
  const savedConfigRevisionRef = useRef(snapshot.configRevision);
  const dirtyRef = useRef(false);
  const recordingShortcutRef = useRef<string | undefined>(undefined);
  const closeRequestRef = useRef<() => Promise<unknown>>(async () => false);
  const panelRef = useRef<HTMLElement>(null);
  const avatarInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const category = settingsCategoryForFocus(focusSection, snapshot.onboarding.settingsHighlight);
    if (category !== undefined) setActiveCategory(category);
  }, [focusSection, snapshot.onboarding.settingsHighlight]);
  useEffect(() => {
    const highlight = focusSection ?? snapshot.onboarding.settingsHighlight;
    if (highlight === undefined) return;
    const reveal = (): void => {
      document.getElementById(highlight === "watch" ? "settings-watch-targets" : "settings-companion")?.scrollIntoView({ behavior: "auto", block: "center" });
    };
    const frame = requestAnimationFrame(reveal);
    const settled = window.setTimeout(reveal, 220);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(settled);
    };
  }, [focusSection, snapshot.onboarding.settingsHighlight]);
  useEffect(() => { formRef.current = form; }, [form]);
  useEffect(() => { recordingShortcutRef.current = recordingShortcut; }, [recordingShortcut]);
  const setShortcutRecording = (value: string | undefined): void => {
    recordingShortcutRef.current = value;
    setRecordingShortcut(value);
  };
  useEffect(() => {
    if (!dirty && !saving) {
      savedConfigRef.current = snapshot.config;
      savedConfigRevisionRef.current = snapshot.configRevision;
      const next = toForm(snapshot.config, snapshot.avatarImageLoadFailed);
      setForm(next);
      formRef.current = next;
    }
  }, [snapshot.config, snapshot.avatarImageLoadFailed, dirty, saving]);
  useEffect(() => {
    if (!dirty && !saving && snapshot.lastError?.kind === "config") setIssues(snapshot.lastError.issues ?? []);
  }, [snapshot.lastError, dirty, saving]);
  useEffect(() => {
    void onAppearancePreview(appearancePreview(form));
  }, [form.uiTheme, form.uiFont, form.avatarColor, form.bubblePosition, form.bubbleDisplay, onAppearancePreview]);

  const update = <K extends keyof FormState>(key: K, value: FormState[K]): void => {
    setSaved(false);
    setForm((current) => {
      const next = { ...current, [key]: value };
      formRef.current = next;
      const nextDirty = hasDraftChanges(savedConfigRef.current, next);
      dirtyRef.current = nextDirty;
      setDirty(nextDirty);
      return next;
    });
  };
  const applyResult = (result: IpcResult<CooSenpaiConfig>, avatarImageLoadFailed: boolean): boolean => {
    if (result.ok) {
      setIssues(result.issues ?? []);
      setExternalChanges(undefined);
      savedConfigRef.current = result.value;
      savedConfigRevisionRef.current = result.value.revision;
      const canonical = toForm(result.value, avatarImageLoadFailed);
      setForm(canonical);
      formRef.current = canonical;
      setDirty(false);
      dirtyRef.current = false;
      setSaved(true);
      window.setTimeout(() => setSaved(false), 1500);
    } else {
      setIssues(result.error.issues ?? [{ path: "config", message: result.error.message }]);
    }
    return result.ok;
  };
  const reloadConflictedDraft = async (draft: FormState, localPatch: ConfigPatch): Promise<void> => {
    const result = await onReloadConfig();
    if (!result.ok) {
      setIssues([{ path: "config", message: `${CONFIG_REVISION_CONFLICT_MESSAGE} ${result.error.message}` }]);
      return;
    }
    const latest = result.value;
    const changed = changedConfigPaths(savedConfigRef.current, latest);
    const rebased = mergeConfigPatch(latest, localPatch);
    const rebasedForm = toForm(rebased, false);
    const next = draft.avatarImage === undefined
      ? rebasedForm
      : { ...rebasedForm, avatarImage: draft.avatarImage, avatarFileName: draft.avatarFileName };
    savedConfigRef.current = latest;
    savedConfigRevisionRef.current = latest.revision;
    formRef.current = next;
    setForm(next);
    const nextDirty = hasDraftChanges(latest, next);
    dirtyRef.current = nextDirty;
    setDirty(nextDirty);
    setExternalChanges(changed);
    setIssues([{ path: "config", message: CONFIG_REVISION_CONFLICT_MESSAGE }]);
  };
  const applySettings = async (): Promise<boolean> => {
    if (!dirtyRef.current || saving) return true;
    setSaving(true);
    const draft = formRef.current;
    const baseConfigRevision = savedConfigRevisionRef.current;
    const patch = changedConfigPatch(savedConfigRef.current, toPatch(draft));
    if (draft.avatarImage !== undefined) {
      patch.ui = {
        ...(patch.ui as Record<string, unknown> | undefined),
        avatarPath: AVATAR_CONFIG_PATH,
      };
    }
    const hasChanges = Object.keys(patch).length !== 0 || draft.avatarImage !== undefined;
    const result = !hasChanges
      ? { ok: true as const, value: savedConfigRef.current }
      : await onSave(patch, draft.avatarImage, baseConfigRevision);
    const success = applyResult(
      result,
      draft.avatarImage === undefined && draft.avatarPath !== null
        ? snapshot.avatarImageLoadFailed
      : false,
    );
    if (!success && !result.ok && isConfigRevisionConflict(result.error.message)) {
      await reloadConflictedDraft(draft, patch);
    }
    setSaving(false);
    return success;
  };
  const selectAvatar = async (event: ChangeEvent<HTMLInputElement>): Promise<void> => {
    const file = event.currentTarget.files?.[0];
    if (file === undefined) return;
    if (file.size > MAX_AVATAR_UPLOAD_BYTES) {
      setIssues([{ path: "ui.avatarPath", message: "画像は 20 MiB 以下にしてください。" }]);
      event.currentTarget.value = "";
      return;
    }
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      setSaved(false);
      setForm((current) => {
        const next = { ...current, avatarPath: AVATAR_CONFIG_PATH, avatarImage: bytes, avatarFileName: file.name, avatarImageLoadFailed: false };
        formRef.current = next;
        dirtyRef.current = true;
        setDirty(true);
        return next;
      });
      setIssues((current) => current.filter((issue) => issue.path !== "ui.avatarPath"));
    } catch {
      setIssues([{ path: "ui.avatarPath", message: "画像ファイルを読み込めませんでした。" }]);
      event.currentTarget.value = "";
    }
  };
  const resetAvatar = (): void => {
    if (saving) return;
    setSaved(false);
    setForm((current) => {
      const next = { ...current, avatarPath: null, avatarImage: undefined, avatarFileName: undefined, avatarImageLoadFailed: false };
      formRef.current = next;
      const nextDirty = hasDraftChanges(savedConfigRef.current, next);
      dirtyRef.current = nextDirty;
      setDirty(nextDirty);
      return next;
    });
    if (avatarInputRef.current !== null) avatarInputRef.current.value = "";
    setIssues((current) => current.filter((issue) => issue.path !== "ui.avatarPath"));
  };
  const changeProvider = (target: "observer" | "companion", provider: ProviderName): void => {
    const model = modelAfterProviderChange(provider, providerModels);
    if (model === undefined) {
      const path = target === "observer" ? "observer.provider" : "companion.provider";
      setIssues((current) => [...current.filter((issue) => issue.path !== path), { path, message: unavailableProviderMessage(providerModelsError) }]);
      return;
    }
    setForm((current) => {
      const next = target === "observer" ? { ...current, providerObserver: provider, observerModel: model } : { ...current, providerCompanion: provider, companionModel: model };
      formRef.current = next;
      const nextDirty = hasDraftChanges(savedConfigRef.current, next);
      dirtyRef.current = nextDirty;
      setDirty(nextDirty);
      setSaved(false);
      return next;
    });
  };
  const requestClose = async (): Promise<boolean> => {
    if (!shouldCloseSettingsDraft(dirtyRef.current, () => { setDiscardConfirmOpen(true); return false; })) return false;
    const cleared = await onAppearancePreview(undefined);
    if (!cleared.ok) {
      setIssues([{ path: "ui", message: cleared.error.message }]);
      return false;
    }
    onClose();
    return true;
  };
  const discardAndClose = async (): Promise<void> => {
    setDiscardConfirmOpen(false);
    const cleared = await onAppearancePreview(undefined);
    if (!cleared.ok) {
      setIssues([{ path: "ui", message: cleared.error.message }]);
      return;
    }
    onClose();
  };
  closeRequestRef.current = requestClose;
  useEffect(() => {
    if (panelRef.current !== null) focusFirstSettingsControl(panelRef.current);
    const listener = createSettingsEscapeListener(() => recordingShortcutRef.current !== undefined, () => setShortcutRecording(undefined), () => closeRequestRef.current());
    document.addEventListener("keydown", listener);
    return () => document.removeEventListener("keydown", listener);
  }, []);
  useEffect(() => () => { void onAppearancePreview(undefined); }, [onAppearancePreview]);

  const resetTuning = (): void => setConfirmation("tuning");
  const applyTuningReset = (): void => {
    setConfirmation(undefined);
    setForm((current) => {
      const next = { ...current, ...defaultTuningForm() };
      formRef.current = next;
      const nextDirty = hasDraftChanges(savedConfigRef.current, next);
      dirtyRef.current = nextDirty;
      setDirty(nextDirty);
      return next;
    });
    setIssues([]);
  };
  const resetConversation = async (): Promise<void> => {
    setConfirmation(undefined);
    await onResetConversation();
  };
  const editPersona = (): void => {
    void onGetPersona(form.persona).then((result) => result.ok ? setPersonaDocument(result.value) : setIssues([{ path: "companion.persona", message: result.error.message }]));
  };
  const focusIssue = (path: string): void => {
    const target = settingsIssueTarget(path);
    setActiveCategory(settingsCategoryForIssue(path));
    const reveal = (): void => {
      const element = document.getElementById(target.id);
      element?.scrollIntoView({ behavior: "smooth", block: "center" });
      const control = element?.matches("input, select, textarea, button") === true ? element as HTMLElement : element?.querySelector<HTMLElement>("input, select, textarea, button");
      control?.focus();
    };
    requestAnimationFrame(reveal);
    window.setTimeout(reveal, 0);
  };
  const errorFor = (path: string): string | undefined => issues.find((issue) => issue.path === path)?.message;
  const categoryProps = { form, snapshot, saving, update, errorFor };
  const generalProps: ComponentProps<typeof GeneralSettings> = { ...categoryProps, personas, avatarInputRef, onSelectAvatar: (event) => { void selectAvatar(event); }, onResetAvatar: resetAvatar, onReloadPersona: () => { void onReloadPersona(); }, onEditPersona: editPersona };
  const providerProps: ComponentProps<typeof ProviderSettings> = { ...categoryProps, providerModels, providerApiKeys, providerApiKeysError, onChangeProvider: changeProvider, onSaveProviderApiKey, onDeleteProviderApiKey };
  const showTutorialPersonaSettings = isTutorialPersonaSettings(snapshot.onboarding);
  const renderCategory = (): ReactElement => {
    switch (activeCategory) {
      case "general":
        return showTutorialPersonaSettings ? <TutorialPersonaSettings general={generalProps} provider={providerProps} /> : <GeneralSettings {...generalProps} />;
      case "vision":
        return <VisionSettings {...categoryProps} highlight={focusSection === "watch"} onOpenSystemSettings={onOpenSystemSettings} onRelaunch={onRelaunch} onResetTuning={resetTuning} />;
      case "hearing":
        return <HearingSettings {...categoryProps} onOpenSpeechSettings={onOpenSpeechSettings} />;
      case "speech":
        return <SpeechSettings {...categoryProps} />;
      case "notifications":
        return <NotificationSettings {...categoryProps} />;
      case "providers":
        return <ProviderSettings {...providerProps} />;
      case "shortcuts":
        return <ShortcutsSettings {...categoryProps} recordingShortcut={recordingShortcut} setShortcutRecording={setShortcutRecording} />;
      case "setup":
        return <SetupSettings {...categoryProps} onRestartTutorial={() => { void onRestartTutorial(); }} onRestartSetup={() => { void onRestartSetup(); }} onResetConversation={() => setConfirmation("conversation-reset")} />;
    }
  };

  return <div className="settings-overlay"><section ref={panelRef} className="settings-panel" aria-label="設定">
    <div className="settings-heading"><div><span>設定</span><h2>CooSenpAI</h2></div><div className="settings-heading-actions"><span className={issues.length > 0 ? "save-state error-text" : "save-state"}>{saving ? "反映中…" : issues.length > 0 ? "反映できていません" : dirty ? "未反映の変更があります" : saved ? "反映しました" : ""}</span><button className="icon-button" type="button" aria-label="設定を閉じる" onClick={() => void requestClose()}><CloseIcon /></button></div></div>
    {issues.length === 0 && snapshot.lastError?.message === undefined && externalChanges === undefined ? null : <div className="settings-error-summary" role="alert"><strong>{settingsIssueHeading(issues.length > 0)}</strong><span>{issues[0]?.message ?? snapshot.lastError?.message}</span>{externalChanges === undefined ? null : <span>別の場所で変更された項目: {externalChanges.length === 0 ? "設定値の差分はありません" : externalChanges.join("、")}</span>}{issues[0] === undefined ? null : <button type="button" onClick={() => focusIssue(issues[0]?.path ?? "config")}>該当する項目を確認</button>}</div>}
    <div className="settings-layout">
      <SettingsTabs activeCategory={activeCategory} onSelect={setActiveCategory} />
      <div id={`settings-category-${activeCategory}`} className="settings-category-panel" role="tabpanel" aria-labelledby={`settings-tab-${activeCategory}`}>
        <form onKeyDown={(event) => { if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.currentTarget.querySelector<HTMLElement>(":focus")?.blur(); }}>
          {renderCategory()}
          {issues.map((issue) => <p className="error-text" key={issue.path}>{configIssueLabel(issue.path)}: {issue.message}</p>)}
          <div className="settings-footer">{dirty ? <button type="button" disabled={saving} onClick={() => void applySettings()}>{saving ? "反映中…" : "反映する"}</button> : <span /> }<span>{saving ? "反映中…" : saved ? "反映しました" : issues.length > 0 ? "反映できていません" : ""}</span></div>
        </form>
      </div>
    </div>
  </section>{discardConfirmOpen ? <SettingsDiscardDialog onCancel={() => setDiscardConfirmOpen(false)} onConfirm={discardAndClose} /> : null}{confirmation === "tuning" ? <ConfirmationDialog id="settings-tuning-reset" title="見るタイミングを既定に戻しますか？" description="間隔・OCR・送信上限を既定値に戻します。" cancelLabel="キャンセル" confirmLabel="既定に戻す" onCancel={() => setConfirmation(undefined)} onConfirm={applyTuningReset} /> : confirmation === "conversation-reset" ? <ConfirmationDialog id="settings-conversation-reset" title="会話をリセットしますか？" description="表示を白紙にして、新しい会話を始めます。履歴は残ります。" cancelLabel="キャンセル" confirmLabel="リセットする" onCancel={() => setConfirmation(undefined)} onConfirm={() => { void resetConversation(); }} /> : null}{personaDocument === undefined ? null : <PersonaEditor option={personas.find((option) => option.id === personaDocument.id) ?? { id: personaDocument.id, displayName: personaDocument.id, builtin: personaDocument.builtin }} document={personaDocument} onSave={onSavePersona} onDelete={onDeletePersona} onRestore={onRestorePersona} onClose={() => setPersonaDocument(undefined)} />}</div>;
}

export function configIssueLabel(path: string): string {
  if (path.startsWith("companion.")) return "会話 AI";
  if (path.startsWith("observer.")) return "Vision AI";
  if (path.startsWith("watch.")) return "Vision AI の撮影";
  if (path.startsWith("audio.")) return "Hearing AI";
  if (path.startsWith("memory.")) return "記憶";
  if (path.startsWith("speech.")) return "音声入力";
  if (path.startsWith("keymap.")) return "ショートカット";
  if (path.startsWith("popup.")) return "通知と吹き出し";
  return "設定";
}
