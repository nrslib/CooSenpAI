import { Fragment, lazy, Suspense, useEffect, useRef, useState, type ChangeEvent, type ComponentProps, type ReactElement } from "react";

import type { AppSnapshot, ConfigIssue, ConfigPatch, CooSenpaiConfig, IpcResult, LicenseDocument, PersonaDocument, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName, SpeakerManagementPayload, SpeakerDirectory, SpeakerRenamePayload } from "./types.js";
import { desktopApi } from "./ipc.js";
import { t, useI18n, type Locale } from "./i18n/index.js";
import { ConfirmationDialog } from "./components/ConfirmationDialog.js";
import { GeneralSettings } from "./components/GeneralSettings.js";
import { HearingSettings } from "./components/HearingSettings.js";
import { DeveloperSettings } from "./components/DeveloperSettings.js";
import { NotificationSettings } from "./components/NotificationSettings.js";
import { ProviderSettings } from "./components/ProviderSettings.js";
import type { ProviderApiKeyDrafts } from "./components/ProviderApiKeyFields.js";
import { SettingsTabs } from "./components/SettingsTabs.js";
import { SettingsDefaultsProvider } from "./components/SettingsControls.js";
import { SettingsDiscardDialog } from "./components/SettingsDiscardDialog.js";
import { SetupSettings } from "./components/SetupSettings.js";
import { WorkSettings } from "./components/WorkSettings.js";
import { ShortcutsSettings } from "./components/ShortcutsSettings.js";
import { SpeechSettings } from "./components/SpeechSettings.js";
import { TutorialPersonaSettings } from "./components/TutorialPersonaSettings.js";
import { VisionSettings } from "./components/VisionSettings.js";
import { CloseIcon } from "./components/LineIcons.js";
import { PersonaEditor } from "./components/PersonaEditor.js";
import { PersonaPicker } from "./components/PersonaPicker.js";
import { companionProviderChange, modelAfterProviderChange, settingsIssueHeading, settingsIssueTarget, unavailableProviderMessage } from "./settings-model.js";
import { focusFirstSettingsControl } from "./settings-keyboard.js";
import { SETTINGS_CATEGORIES, type SettingsCategory } from "./settings-categories.js";
import { appearancePreview, resetSettingsForm, restoreSettingsForm, sameFormValue, toForm, type ConfigFormKey, type FormState, type ResetFormValues, type SettingsAppearancePreview, type SettingsResetRequest } from "./settings-form.js";
import { resetFields, type SettingsResetFields } from "./settings-form-fields.js";
import { useSettingsPresenter } from "./useSettingsPresenter.js";
import { SettingsFormSync } from "./settings-form-sync.js";
import { SettingsSearchCategory, SettingsSearchItem, SettingsSearchProvider } from "./settings-search.js";

const VrmControls = lazy(() => import("./vrm/VrmControls.js"));

export type { SettingsAppearancePreview } from "./settings-form.js";
export { resetShortcutToDefault, shortcutFromKeyboardEvent } from "./components/ShortcutsSettings.js";

const AVATAR_CONFIG_PATH = "state/avatar.png";
const MAX_AVATAR_UPLOAD_BYTES = 20 * 1024 * 1024;
const MAX_DEBUG_WAKE_BYTES = 32 * 1024 * 1024;

function isSupportedDebugWakeImage(file: File): boolean {
  if (file.type === "image/png" || file.type === "image/jpeg") return true;
  if (file.type !== "") return false;
  const name = file.name.toLowerCase();
  return name.endsWith(".png") || name.endsWith(".jpg") || name.endsWith(".jpeg");
}

function settingsResetRequest(scope: SettingsResetRequest["scope"] | null, category: SettingsCategory | null): SettingsResetRequest {
  switch (scope) {
    case "page":
      if (category === null) throw new Error("Missing settings page");
      return { scope, category };
    case "all":
      return { scope };
    case null:
      throw new Error("Missing settings reset scope");
  }
}

function resetFormValues(fields: SettingsResetFields): ResetFormValues {
  return Object.fromEntries(Object.entries(fields).map(([key, field]) => {
    if (field === undefined) throw new Error("Missing reset setting value");
    return [key, field.value];
  })) as ResetFormValues;
}

interface ResetUndoTracking {
  readonly keys: readonly ConfigFormKey[];
  readonly manuallyChanged: Set<ConfigFormKey>;
}

interface Props {
  readonly snapshot: AppSnapshot;
  readonly personas: readonly PersonaOption[];
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerModelsError?: string;
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly focusSection?: "watch" | "providers";
  readonly focusGeneration?: number;
  readonly onClose: () => void;
  readonly onSave: (patch: ConfigPatch, avatarImage: readonly number[] | undefined, baseConfigRevision: number) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onSelectPersona: (persona: string) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onReloadConfig: () => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onReloadPersona: () => Promise<IpcResult<null>>;
  readonly onGetPersona: (id: string) => Promise<IpcResult<PersonaDocument>>;
  readonly onSavePersona: (id: string, displayName: string, body: string) => Promise<IpcResult<unknown>>;
  readonly onDeletePersona: (id: string) => Promise<IpcResult<unknown>>;
  readonly onRefreshPersonas: () => Promise<IpcResult<unknown>>;
  readonly onRestorePersona: (id: string, version: string) => Promise<IpcResult<unknown>>;
  readonly onRestartTutorial: () => Promise<IpcResult<unknown>>;
  readonly onRestartSetup: () => Promise<IpcResult<unknown>>;
  readonly onResetConversation: () => Promise<IpcResult<unknown>>;
  readonly onOpenSystemSettings: () => void;
  readonly onOpenAccessibilitySettings?: () => void;
  readonly onOpenLicenseDocument: (document: LicenseDocument) => void;
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
  readonly onSpeakerManagement?: (payload: SpeakerManagementPayload) => Promise<IpcResult<null>>;
  readonly onSpeakerDirectory?: () => Promise<IpcResult<SpeakerDirectory>>;
  readonly onSpeakerRename?: (payload: SpeakerRenamePayload) => Promise<IpcResult<SpeakerDirectory>>;
  readonly onToggleAvatar: () => void;
  readonly onRelaunch: () => void;
  readonly onAppearancePreview: (preview?: SettingsAppearancePreview) => Promise<IpcResult<null>>;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export { SettingsDiscardDialog } from "./components/SettingsDiscardDialog.js";

export function SettingsPanel({ snapshot, personas, providerModels, providerModelsError, providerApiKeys, providerApiKeysError, focusSection, focusGeneration = 0, onClose, onSave, onSelectPersona, onReloadConfig, onReloadPersona, onGetPersona, onSavePersona, onDeletePersona, onRestorePersona, onRefreshPersonas, onRestartTutorial, onRestartSetup, onResetConversation, onOpenSystemSettings, onOpenAccessibilitySettings, onOpenLicenseDocument, onOpenSpeechSettings, onSpeakerManagement, onSpeakerDirectory, onSpeakerRename, onToggleAvatar, onRelaunch, onAppearancePreview, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  const { locale, t } = useI18n();
  const [formSync] = useState(() => new SettingsFormSync(toForm(snapshot.config, snapshot.avatarImageLoadFailed), snapshot.configRevision));
  const [form, setForm] = useState(formSync.form);
  const [settingsQuery, setSettingsQuery] = useState("");
  const [providerApiKeyDrafts, setProviderApiKeyDrafts] = useState<ProviderApiKeyDrafts>({ codex: "", claude: "", opencode: "" });
  const [issueFocusRequest, setIssueFocusRequest] = useState<{ readonly path: string }>();
  const panelRef = useRef<HTMLElement>(null);
  const avatarInputRef = useRef<HTMLInputElement>(null);
  const resetUndoTracking = useRef<ResetUndoTracking | undefined>(undefined);
  const focusHandledGeneration = useRef<number | undefined>(undefined);
  const presenter = useSettingsPresenter(snapshot, focusSection, focusGeneration, formSync.input(), {
    speakerManagement: onSpeakerManagement ?? (async () => ({ ok: false, error: { message: "Speaker management unavailable" } })),
    speakerDirectory: onSpeakerDirectory ?? (async () => ({ ok: false, error: { message: "Speaker directory unavailable" } })),
    speakerRename: onSpeakerRename ?? (async () => ({ ok: false, error: { message: "Speaker names unavailable" } })),
    feedbackExport: desktopApi.exportUtteranceFeedback,
    debugWake: desktopApi.debugWake,
    save: onSave, selectPersona: onSelectPersona, getPersona: onGetPersona, reload: onReloadConfig,
    resetConversation: onResetConversation, clearPreview: () => onAppearancePreview(undefined), close: onClose,
    reflect: (value) => {
      if (formSync.reflect(value)) setForm(formSync.form);
    },
    resetDraft: (payload) => {
      const request = settingsResetRequest(payload.scope, payload.category);
      edit(resetSettingsForm(formSync.form, payload.defaults, request), false);
      resetUndoTracking.current = {
        keys: Object.keys(payload.fields) as ConfigFormKey[],
        manuallyChanged: new Set(),
      };
    },
    restoreReset: (payload) => {
      const request = settingsResetRequest(payload.scope, payload.category);
      const manuallyChanged = resetUndoTracking.current?.manuallyChanged ?? new Set<ConfigFormKey>();
      resetUndoTracking.current = undefined;
      edit(restoreSettingsForm(formSync.form, resetFormValues(payload.fields), payload.defaults, request, manuallyChanged), false);
    },
    focusIssue: (path) => { setSettingsQuery(""); setIssueFocusRequest({ path }); },
    focusFirst: () => { if (panelRef.current !== null) focusFirstSettingsControl(panelRef.current); },
  });
  const action = (name: string, value: unknown = null): void => presenter.send({ type: "action", name, value: name === "save" || name === "close" ? formSync.input() : value });
  const selectDebugWakeImage = async (file: File | undefined): Promise<void> => {
    if (file === undefined) return;
    action("debugWakeLoading");
    if (file.size > MAX_DEBUG_WAKE_BYTES) {
      action("debugWakeSelectionError", t("settings.developer.debugWakeTooLarge"));
      return;
    }
    if (!isSupportedDebugWakeImage(file)) {
      action("debugWakeSelectionError", t("settings.developer.debugWakeUnsupported"));
      return;
    }
    try {
      const image = Array.from(new Uint8Array(await file.arrayBuffer()));
      action("debugWakeSelect", { name: file.name, image });
    } catch {
      action("debugWakeSelectionError", t("settings.developer.debugWakeReadFailed"));
    }
  };
  const edit = (next: FormState, trackResetChanges = true): void => {
    if (trackResetChanges) {
      const tracking = resetUndoTracking.current;
      if (tracking !== undefined) {
        for (const key of tracking.keys) {
          if (!sameFormValue(formSync.form[key], next[key])) tracking.manuallyChanged.add(key);
        }
      }
    }
    formSync.edit(next);
    setForm(next);
    presenter.send({ type: "change", value: formSync.input() });
  };
  const state = presenter.state;
  const debugWake = state?.debugWake ?? { phase: "idle" as const, selectedName: null, context: "", result: null, error: null };
  const issues = state?.issues ?? [];
  const saving = state === undefined || state.saving || state.closing;
  const saved = state?.saved === true;
  const dirty = state?.dirty === true;
  const externalChanges = state?.externalChanges ?? undefined;
  const discardConfirmOpen = state?.discardConfirmOpen === true;
  const confirmation = state?.confirmation;
  const resetScope = state?.resetScope ?? null;
  const resetCategory = state?.resetCategory ?? null;
  const canUndoReset = state?.canUndoReset === true;
  const recordingShortcut = state?.recordingShortcut ?? undefined;
  const personaDocument = state?.personaDocument ?? undefined;
  const activeCategory = state?.activeCategory ?? "general";
  const vrmControlsOpen = state?.vrmControlsOpen === true;
  const showTutorialPersonaSettings = state?.showTutorialPersonaSettings === true;
  const setShortcutRecording = (value: string | undefined): void => action("recording", value ?? null);
  const update = <K extends keyof FormState>(key: K, value: FormState[K]): void => edit({ ...formSync.form, [key]: value });
  const setIssues = (value: readonly ConfigIssue[]): void => action("issues", value);
  useEffect(() => { void onAppearancePreview(appearancePreview(form)); },
    [form.uiTheme, form.uiFont, form.avatarColor, form.bubblePosition, form.bubbleDisplay, onAppearancePreview]);
  useEffect(() => () => { void onAppearancePreview(undefined); }, [onAppearancePreview]);
  useEffect(() => {
    const listener = (event: KeyboardEvent): void => {
      if (event.key === "Escape" && !event.isComposing && event.keyCode !== 229 && presenter.state?.escapeEnabled === true) {
        event.preventDefault();
        event.stopPropagation();
      }
      presenter.send({ type: "action", name: "escape", value: { key: event.key, composing: event.isComposing, keyCode: event.keyCode, draft: formSync.input() } });
    };
    document.addEventListener("keydown", listener);
    return () => document.removeEventListener("keydown", listener);
  }, [presenter.send, presenter.state?.escapeEnabled, formSync]);
  useEffect(() => {
    const highlight = focusSection ?? snapshot.onboarding.settingsHighlight;
    if (highlight === undefined) return;
    const targetId = highlight === "watch" ? "settings-watch-targets" : highlight === "providers" ? "settings-ai" : "settings-companion";
    const frame = requestAnimationFrame(() => {
      const target = document.getElementById(targetId);
      if (target === null || focusSection !== undefined && focusHandledGeneration.current === focusGeneration) return;
      target.scrollIntoView({ behavior: "auto", block: "center" });
      if (focusSection !== undefined) focusHandledGeneration.current = focusGeneration;
    });
    return () => cancelAnimationFrame(frame);
  }, [focusSection, focusGeneration, snapshot.onboarding.settingsHighlight, activeCategory]);
  const selectAvatar = async (event: ChangeEvent<HTMLInputElement>): Promise<void> => {
    const file = event.currentTarget.files?.[0];
    if (file === undefined) return;
    const input = event.currentTarget;
    if (file.size > MAX_AVATAR_UPLOAD_BYTES) { setIssues([{ path: "ui.avatarPath", message: t("settings.avatarTooLarge") }]); input.value = ""; return; }
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      edit({ ...formSync.form, avatarPath: AVATAR_CONFIG_PATH, avatarImage: bytes, avatarFileName: file.name, avatarImageLoadFailed: false });
      setIssues(issues.filter((issue) => issue.path !== "ui.avatarPath"));
    } catch { setIssues([{ path: "ui.avatarPath", message: t("settings.avatarReadFailed") }]); input.value = ""; }
  };
  const resetAvatar = (): void => {
    edit({ ...formSync.form, avatarPath: null, avatarImage: undefined, avatarFileName: undefined, avatarImageLoadFailed: false });
    if (avatarInputRef.current !== null) avatarInputRef.current.value = "";
    setIssues(issues.filter((issue) => issue.path !== "ui.avatarPath"));
  };
  const changeProvider = (target: "vision" | "hearing" | "companion", provider: ProviderName): void => {
    const model = modelAfterProviderChange(provider, providerModels);
    if (model === undefined) {
      const path = target === "vision" ? "observer.vision.provider" : target === "hearing" ? "observer.hearing.provider" : "companion.provider";
      setIssues([...issues.filter((issue) => issue.path !== path), { path, message: unavailableProviderMessage(providerModelsError, locale) }]);
      return;
    }
    const current = formSync.form;
    if (target === "vision") edit({ ...current, providerObserver: provider, observerModel: model });
    else if (target === "hearing") edit({ ...current, providerHearing: provider, hearingModel: model });
    else edit({ ...current, ...companionProviderChange(provider, model) });
  };
  const requestClose = (): void => action("close");
  const discardAndClose = async (): Promise<void> => action("discard");
  const resetPage = (category: SettingsCategory): void => action("confirm", { scope: "page", category });
  const resetAll = (): void => action("confirm", { scope: "all" });
  const undoReset = (): void => action("undoReset");
  const resetRequestForConfirmation = (): SettingsResetRequest => {
    if (confirmation === "settings-reset") return settingsResetRequest(resetScope, resetCategory);
    throw new Error("No settings reset confirmation");
  };
  const acceptReset = (): void => {
    action("acceptConfirmation", { fields: resetFields(formSync.form, resetRequestForConfirmation()) });
  };
  const editPersona = (): void => action("editPersona", formSync.form.persona);
  const selectPersona = (persona: string): void => action("selectPersona", persona);
  const focusIssue = (path: string): void => action("focusIssue", path);
  useEffect(() => {
    const request = issueFocusRequest;
    if (request === undefined || settingsQuery.trim() !== "") return;
    const target = settingsIssueTarget(request.path);
    const reveal = (): void => {
      const element = document.getElementById(target.id);
      element?.scrollIntoView({ behavior: "smooth", block: "center" });
      const control = element?.matches("input, select, textarea, button") === true ? element as HTMLElement : element?.querySelector<HTMLElement>("input, select, textarea, button");
      control?.focus();
    };
    const frame = requestAnimationFrame(reveal);
    const settled = window.setTimeout(() => {
      reveal();
      setIssueFocusRequest(undefined);
    }, 0);
    return () => {
      cancelAnimationFrame(frame);
      window.clearTimeout(settled);
    };
  }, [issueFocusRequest, settingsQuery]);
  const errorFor = (path: string): string | undefined => issues.find((issue) => issue.path === path)?.message;
  const searching = settingsQuery.trim() !== "";
  const updateProviderApiKeyDraft = (provider: ProviderName, value: string): void => {
    setProviderApiKeyDrafts((current) => ({ ...current, [provider]: value }));
  };
  const categoryProps = { form, snapshot, saving, update, errorFor };
  const openPersonaPicker = (): void => {
    action("openPicker");
  };
  const generalProps: ComponentProps<typeof GeneralSettings> = { ...categoryProps, feedbackExport: presenter.state?.feedbackExport, onFeedbackExport: () => presenter.send({ type: "action", name: "feedbackExport", value: null }), personas, avatarInputRef, onSelectAvatar: (event) => { void selectAvatar(event); }, onResetAvatar: resetAvatar, onReloadPersona: () => { void onReloadPersona(); }, onEditPersona: editPersona, onOpenPersonaPicker: openPersonaPicker };
  const providerProps: ComponentProps<typeof ProviderSettings> = { ...categoryProps, providerModels, providerApiKeys, providerApiKeysError, providerApiKeyDrafts, onChangeProvider: changeProvider, onProviderApiKeyDraftChange: updateProviderApiKeyDraft, onSaveProviderApiKey, onDeleteProviderApiKey };
  const renderCategory = (category: SettingsCategory): ReactElement => {
    let content: ReactElement;
    switch (category) {
      case "general":
        content = !searching && showTutorialPersonaSettings ? <TutorialPersonaSettings general={generalProps} provider={providerProps} /> : <GeneralSettings {...generalProps} />;
        break;
      case "vision":
        content = <VisionSettings {...categoryProps} highlight={focusSection === "watch"} onOpenSystemSettings={onOpenSystemSettings} onOpenAccessibilitySettings={onOpenAccessibilitySettings} onRelaunch={onRelaunch} />;
        break;
      case "hearing":
        content = <HearingSettings {...categoryProps} onOpenSpeechSettings={onOpenSpeechSettings} speaker={state?.speakerManagement} speakerConfirmation={confirmation === "speaker-delete" || confirmation === "speaker-delete-all" ? confirmation : null} onSpeakerAction={action} />;
        break;
      case "speech":
        content = <SpeechSettings {...categoryProps} />;
        break;
      case "notifications":
        content = <NotificationSettings {...categoryProps} />;
        break;
      case "providers":
        content = <ProviderSettings {...providerProps} />;
        break;
      case "shortcuts":
        content = <ShortcutsSettings {...categoryProps} recordingShortcut={recordingShortcut} setShortcutRecording={setShortcutRecording} />;
        break;
      case "setup":
        content = <SetupSettings {...categoryProps} onRestartTutorial={() => { void onRestartTutorial(); }} onRestartSetup={() => { void onRestartSetup(); }} onResetConversation={() => action("confirm", "conversation-reset")} onOpenLicenseDocument={onOpenLicenseDocument} />;
        break;
      case "work":
        content = <WorkSettings {...categoryProps} />;
        break;
      case "developer":
        content = <>
          <DeveloperSettings
            {...categoryProps}
            debugWake={debugWake}
            onDebugWakeImage={(file) => { void selectDebugWakeImage(file); }}
            onDebugWakeContext={(value) => action("debugWakeContext", value)}
            onDebugWakeSend={() => action("debugWakeSend")}
          />
          <SettingsSearchItem label={t("app.vrmMenu")} description={t("settings.categories.developer")}>
            <fieldset id="settings-vrm"><legend>{t("app.vrmMenu")}</legend>
              <div className="button-row">
                <button type="button" onClick={() => action("openVrm")}>{t("app.vrmMenu")}</button>
                <button type="button" onClick={onToggleAvatar}>{t("vrm.avatarToggle")}</button>
              </div>
            </fieldset>
          </SettingsSearchItem>
        </>;
        break;
    }
    const definition = SETTINGS_CATEGORIES.find((candidate) => candidate.id === category);
    if (definition === undefined) throw new Error(`Unknown settings category: ${category}`);
    return <SettingsSearchCategory label={t(definition.labelKey)}>
      {content}
      {searching ? null : <div className="button-row settings-reset-actions">
        <button id={`settings-reset-page-${category}`} type="button" disabled={saving} onClick={() => resetPage(category)}>{t("settings.resetPage")}</button>
      </div>}
    </SettingsSearchCategory>;
  };

  return <div className="settings-overlay"><section ref={panelRef} className="settings-panel" aria-label={t("settings.label")}>
    {presenter.error === undefined ? null : <p role="alert">{presenter.error}</p>}
    <div className="settings-heading"><div className="settings-heading-main"><div><span>{t("settings.label")}</span><h2>CooSenpAI</h2></div><label className="settings-search"><span>{t("settings.search")}</span><input id="settings-search" type="search" value={settingsQuery} placeholder={t("settings.searchPlaceholder")} aria-label={t("settings.search")} onChange={(event) => setSettingsQuery(event.target.value)} /></label></div><div className="settings-heading-actions"><span className={issues.length > 0 ? "save-state error-text" : "save-state"}>{saving ? t("settings.state.saving") : issues.length > 0 ? t("settings.state.notApplied") : dirty ? t("settings.state.unsaved") : saved ? t("settings.state.saved") : ""}</span><button id="settings-reset-all" type="button" disabled={saving} onClick={resetAll}>{t("settings.resetAll")}</button>{canUndoReset ? <button id="settings-reset-undo" type="button" disabled={saving} onClick={undoReset}>{t("settings.undoReset")}</button> : null}<button className="icon-button" type="button" aria-label={t("settings.closeAria")} onClick={() => void requestClose()}><CloseIcon /></button></div></div>
    {issues.length === 0 && snapshot.lastError?.message === undefined && externalChanges === undefined ? null : <div className="settings-error-summary" role="alert"><strong>{settingsIssueHeading(issues.length > 0, locale)}</strong><span>{issues[0]?.message ?? snapshot.lastError?.message}</span>{externalChanges === undefined ? null : <span>{t("settings.externalChangesLabel", { items: externalChanges.length === 0 ? t("settings.noDifference") : externalChanges.join(locale === "ja" ? "、" : ", ") })}</span>}{issues[0] === undefined ? null : <button type="button" onClick={() => focusIssue(issues[0]?.path ?? "config")}>{t("settings.focusIssue")}</button>}</div>}
    <p className="field-help settings-defaults-notice">{t("settings.defaultsResetNotice")}</p>
    <SettingsDefaultsProvider config={state?.defaultConfig}><SettingsSearchProvider query={settingsQuery}><div className="settings-layout">
      <SettingsTabs activeCategory={activeCategory} onSelect={(category) => action("category", category)} disabled={searching} />
      <div id={searching ? "settings-search-results" : `settings-category-${activeCategory}`} className="settings-category-panel" role={searching ? "region" : "tabpanel"} aria-label={searching ? t("settings.searchResultRegion") : undefined} aria-labelledby={searching ? undefined : `settings-tab-${activeCategory}`}>
        <form onKeyDown={(event) => { if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.currentTarget.querySelector<HTMLElement>(":focus")?.blur(); }}>
          {searching ? <div className="settings-search-results" data-settings-search-results="true"><p className="settings-search-summary">{t("settings.searchResultItem", { query: settingsQuery.trim() })}</p>{SETTINGS_CATEGORIES.map((category) => <Fragment key={category.id}>{renderCategory(category.id)}</Fragment>)}</div> : renderCategory(activeCategory)}
          {issues.map((issue) => <p className="error-text" key={issue.path}>{configIssueLabel(issue.path, locale)}: {issue.message}</p>)}
          <div className="settings-footer">{dirty ? <button type="button" disabled={saving} onClick={() => action("save")}>{saving ? t("settings.state.saving") : t("common.apply")}</button> : <span /> }<span>{saving ? t("settings.state.saving") : saved ? t("settings.state.saved") : issues.length > 0 ? t("settings.state.notApplied") : ""}</span></div>
        </form>
      </div>
    </div></SettingsSearchProvider></SettingsDefaultsProvider>
    {vrmControlsOpen ? <Suspense fallback={null}><VrmControls controlsOpen onCloseControls={() => action("closeVrm")} showClosedError={false} /></Suspense> : null}
  </section>{discardConfirmOpen ? <SettingsDiscardDialog onCancel={() => action("cancelDiscard")} onConfirm={discardAndClose} /> : null}{confirmation === "settings-reset" ? <ConfirmationDialog id={resetScope === "all" ? "settings-all-reset" : "settings-page-reset"} title={resetScope === "all" ? t("settings.resetAllTitle") : t("settings.resetPageTitle", { category: resetCategory === null ? "" : t(SETTINGS_CATEGORIES.find((candidate) => candidate.id === resetCategory)!.labelKey) })} description={resetScope === "all" ? t("settings.resetAllDescription") : t("settings.resetPageDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("settings.resetDefault")} onCancel={() => action("cancelConfirmation")} onConfirm={acceptReset} /> : confirmation === "conversation-reset" ? <ConfirmationDialog id="settings-conversation-reset" title={t("app.resetConversationTitle")} description={t("app.resetConversationDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("app.resetConversation")} onCancel={() => action("cancelConfirmation")} onConfirm={() => { action("acceptConfirmation"); }} /> : null}{personaDocument === undefined ? null : <PersonaEditor option={personas.find((option) => option.id === personaDocument.id) ?? { id: personaDocument.id, displayName: personaDocument.id, builtin: personaDocument.builtin }} document={personaDocument} onSave={onSavePersona} onDelete={onDeletePersona} onRestore={onRestorePersona} onRefresh={onRefreshPersonas} onClose={() => action("closePersona")} />}{state?.personaPickerOpen === true ? <PersonaPicker personas={personas} selectedPersona={form.persona} busy={saving} error={errorFor("companion.persona")} onSelect={(persona) => { void selectPersona(persona); }} onClose={() => action("closePicker")} /> : null}</div>;
}

export function configIssueLabel(path: string, locale: Locale = "ja"): string {
  if (path.startsWith("companion.")) return t(locale, "settings.configIssue.companion");
  if (path.startsWith("observer.")) return t(locale, "settings.configIssue.observer");
  if (path.startsWith("watch.")) return t(locale, "settings.configIssue.watch");
  if (path.startsWith("audio.")) return t(locale, "settings.configIssue.audio");
  if (path.startsWith("memory.")) return t(locale, "settings.configIssue.memory");
  if (path.startsWith("speech.")) return t(locale, "settings.configIssue.speech");
  if (path.startsWith("keymap.")) return t(locale, "settings.configIssue.keymap");
  if (path.startsWith("popup.")) return t(locale, "settings.configIssue.popup");
  return t(locale, "settings.configIssue.default");
}
