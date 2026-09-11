import { Fragment, lazy, Suspense, useEffect, useRef, useState, type ChangeEvent, type ComponentProps, type ReactElement } from "react";

import type { AppSnapshot, ConfigIssue, ConfigPatch, CooSenpaiConfig, IpcResult, LicenseDocument, PersonaDocument, PersonaOption, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "./types.js";
import { t, useI18n, type Locale } from "./i18n/index.js";
import { ConfirmationDialog } from "./components/ConfirmationDialog.js";
import { GeneralSettings } from "./components/GeneralSettings.js";
import { HearingSettings } from "./components/HearingSettings.js";
import { VoiceOutputSettings } from "./components/VoiceOutputSettings.js";
import { NotificationSettings } from "./components/NotificationSettings.js";
import { ProviderSettings } from "./components/ProviderSettings.js";
import type { ProviderApiKeyDrafts } from "./components/ProviderApiKeyFields.js";
import { SettingsTabs } from "./components/SettingsTabs.js";
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
import { modelAfterProviderChange, settingsIssueHeading, settingsIssueTarget, unavailableProviderMessage } from "./settings-model.js";
import { focusFirstSettingsControl } from "./settings-keyboard.js";
import { SETTINGS_CATEGORIES, type SettingsCategory } from "./settings-categories.js";
import { appearancePreview, defaultTuningForm, toForm, type FormState, type SettingsAppearancePreview } from "./settings-form.js";
import { useSettingsPresenter } from "./useSettingsPresenter.js";
import { SettingsFormSync } from "./settings-form-sync.js";
import { SettingsSearchCategory, SettingsSearchItem, SettingsSearchProvider } from "./settings-search.js";

const VrmControls = lazy(() => import("./vrm/VrmControls.js"));

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
  readonly onOpenLicenseDocument: (document: LicenseDocument) => void;
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
  readonly onToggleAvatar: () => void;
  readonly onRelaunch: () => void;
  readonly onAppearancePreview: (preview?: SettingsAppearancePreview) => Promise<IpcResult<null>>;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export { SettingsDiscardDialog } from "./components/SettingsDiscardDialog.js";

export function SettingsPanel({ snapshot, personas, providerModels, providerModelsError, providerApiKeys, providerApiKeysError, focusSection, onClose, onSave, onSelectPersona, onReloadConfig, onReloadPersona, onGetPersona, onSavePersona, onDeletePersona, onRestorePersona, onRefreshPersonas, onRestartTutorial, onRestartSetup, onResetConversation, onOpenSystemSettings, onOpenLicenseDocument, onOpenSpeechSettings, onToggleAvatar, onRelaunch, onAppearancePreview, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  const { locale, t } = useI18n();
  const [formSync] = useState(() => new SettingsFormSync(toForm(snapshot.config, snapshot.avatarImageLoadFailed), snapshot.configRevision));
  const [form, setForm] = useState(formSync.form);
  const [settingsQuery, setSettingsQuery] = useState("");
  const [providerApiKeyDrafts, setProviderApiKeyDrafts] = useState<ProviderApiKeyDrafts>({ codex: "", claude: "", opencode: "" });
  const [issueFocusRequest, setIssueFocusRequest] = useState<{ readonly path: string }>();
  const panelRef = useRef<HTMLElement>(null);
  const avatarInputRef = useRef<HTMLInputElement>(null);
  const presenter = useSettingsPresenter(snapshot, focusSection, formSync.input(), {
    save: onSave, selectPersona: onSelectPersona, getPersona: onGetPersona, reload: onReloadConfig,
    resetConversation: onResetConversation, clearPreview: () => onAppearancePreview(undefined), close: onClose,
    reflect: (value) => {
      if (formSync.reflect(value)) setForm(formSync.form);
    },
    resetTuning: () => edit({ ...formSync.form, ...defaultTuningForm() }),
    focusIssue: (path) => { setSettingsQuery(""); setIssueFocusRequest({ path }); },
    focusFirst: () => { if (panelRef.current !== null) focusFirstSettingsControl(panelRef.current); },
  });
  const action = (name: string, value: unknown = null): void => presenter.send({ type: "action", name, value: name === "save" || name === "close" ? formSync.input() : value });
  const edit = (next: FormState): void => {
    formSync.edit(next);
    setForm(next);
    presenter.send({ type: "change", value: formSync.input() });
  };
  const state = presenter.state;
  const issues = state?.issues ?? [];
  const saving = state === undefined || state.saving || state.closing;
  const saved = state?.saved === true;
  const dirty = state?.dirty === true;
  const externalChanges = state?.externalChanges ?? undefined;
  const discardConfirmOpen = state?.discardConfirmOpen === true;
  const confirmation = state?.confirmation;
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
    const frame = requestAnimationFrame(() => {
      document.getElementById(highlight === "watch" ? "settings-watch-targets" : "settings-companion")?.scrollIntoView({ behavior: "auto", block: "center" });
    });
    return () => cancelAnimationFrame(frame);
  }, [focusSection, snapshot.onboarding.settingsHighlight, activeCategory]);
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
    else edit({ ...current, providerCompanion: provider, companionModel: model });
  };
  const requestClose = (): void => action("close");
  const discardAndClose = async (): Promise<void> => action("discard");
  const resetTuning = (): void => action("confirm", "tuning");
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
  const generalProps: ComponentProps<typeof GeneralSettings> = { ...categoryProps, personas, avatarInputRef, onSelectAvatar: (event) => { void selectAvatar(event); }, onResetAvatar: resetAvatar, onReloadPersona: () => { void onReloadPersona(); }, onEditPersona: editPersona, onOpenPersonaPicker: openPersonaPicker };
  const providerProps: ComponentProps<typeof ProviderSettings> = { ...categoryProps, providerModels, providerApiKeys, providerApiKeysError, providerApiKeyDrafts, onChangeProvider: changeProvider, onProviderApiKeyDraftChange: updateProviderApiKeyDraft, onSaveProviderApiKey, onDeleteProviderApiKey, onResetTuning: resetTuning };
  const renderCategory = (category: SettingsCategory): ReactElement => {
    let content: ReactElement;
    switch (category) {
      case "general":
        content = !searching && showTutorialPersonaSettings ? <TutorialPersonaSettings general={generalProps} provider={providerProps} /> : <GeneralSettings {...generalProps} />;
        break;
      case "vision":
        content = <VisionSettings {...categoryProps} highlight={focusSection === "watch"} onOpenSystemSettings={onOpenSystemSettings} onRelaunch={onRelaunch} onResetTuning={resetTuning} />;
        break;
      case "hearing":
        content = <HearingSettings {...categoryProps} onOpenSpeechSettings={onOpenSpeechSettings} />;
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
      case "beta":
        content = <>
          <p className="field-help">{t("settings.beta.notice")}</p>
          <VoiceOutputSettings {...categoryProps} />
          <SettingsSearchItem label={t("app.vrmMenu")} description={t("settings.categories.beta")}>
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
    return <SettingsSearchCategory label={t(definition.labelKey)}>{content}</SettingsSearchCategory>;
  };

  return <div className="settings-overlay"><section ref={panelRef} className="settings-panel" aria-label={t("settings.label")}>
    {presenter.error === undefined ? null : <p role="alert">{presenter.error}</p>}
    <div className="settings-heading"><div className="settings-heading-main"><div><span>{t("settings.label")}</span><h2>CooSenpAI</h2></div><label className="settings-search"><span>{t("settings.search")}</span><input id="settings-search" type="search" value={settingsQuery} placeholder={t("settings.searchPlaceholder")} aria-label={t("settings.search")} onChange={(event) => setSettingsQuery(event.target.value)} /></label></div><div className="settings-heading-actions"><span className={issues.length > 0 ? "save-state error-text" : "save-state"}>{saving ? t("settings.state.saving") : issues.length > 0 ? t("settings.state.notApplied") : dirty ? t("settings.state.unsaved") : saved ? t("settings.state.saved") : ""}</span><button className="icon-button" type="button" aria-label={t("settings.closeAria")} onClick={() => void requestClose()}><CloseIcon /></button></div></div>
    {issues.length === 0 && snapshot.lastError?.message === undefined && externalChanges === undefined ? null : <div className="settings-error-summary" role="alert"><strong>{settingsIssueHeading(issues.length > 0, locale)}</strong><span>{issues[0]?.message ?? snapshot.lastError?.message}</span>{externalChanges === undefined ? null : <span>{t("settings.externalChangesLabel", { items: externalChanges.length === 0 ? t("settings.noDifference") : externalChanges.join(locale === "ja" ? "、" : ", ") })}</span>}{issues[0] === undefined ? null : <button type="button" onClick={() => focusIssue(issues[0]?.path ?? "config")}>{t("settings.focusIssue")}</button>}</div>}
    <SettingsSearchProvider query={settingsQuery}><div className="settings-layout">
      <SettingsTabs activeCategory={activeCategory} onSelect={(category) => action("category", category)} disabled={searching} />
      <div id={searching ? "settings-search-results" : `settings-category-${activeCategory}`} className="settings-category-panel" role={searching ? "region" : "tabpanel"} aria-label={searching ? t("settings.searchResultRegion") : undefined} aria-labelledby={searching ? undefined : `settings-tab-${activeCategory}`}>
        <form onKeyDown={(event) => { if (event.key === "Enter" && event.target instanceof HTMLInputElement) event.currentTarget.querySelector<HTMLElement>(":focus")?.blur(); }}>
          {searching ? <div className="settings-search-results" data-settings-search-results="true"><p className="settings-search-summary">{t("settings.searchResultItem", { query: settingsQuery.trim() })}</p>{SETTINGS_CATEGORIES.map((category) => <Fragment key={category.id}>{renderCategory(category.id)}</Fragment>)}</div> : renderCategory(activeCategory)}
          {issues.map((issue) => <p className="error-text" key={issue.path}>{configIssueLabel(issue.path, locale)}: {issue.message}</p>)}
          <div className="settings-footer">{dirty ? <button type="button" disabled={saving} onClick={() => action("save")}>{saving ? t("settings.state.saving") : t("common.apply")}</button> : <span /> }<span>{saving ? t("settings.state.saving") : saved ? t("settings.state.saved") : issues.length > 0 ? t("settings.state.notApplied") : ""}</span></div>
        </form>
      </div>
    </div></SettingsSearchProvider>
    {vrmControlsOpen ? <Suspense fallback={null}><VrmControls controlsOpen onCloseControls={() => action("closeVrm")} showClosedError={false} /></Suspense> : null}
  </section>{discardConfirmOpen ? <SettingsDiscardDialog onCancel={() => action("cancelDiscard")} onConfirm={discardAndClose} /> : null}{confirmation === "tuning" ? <ConfirmationDialog id="settings-tuning-reset" title={t("settings.resetTuningTitle")} description={t("settings.resetTuningDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("settings.resetDefault")} onCancel={() => action("cancelConfirmation")} onConfirm={() => action("acceptConfirmation")} /> : confirmation === "conversation-reset" ? <ConfirmationDialog id="settings-conversation-reset" title={t("app.resetConversationTitle")} description={t("app.resetConversationDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("app.resetConversation")} onCancel={() => action("cancelConfirmation")} onConfirm={() => { action("acceptConfirmation"); }} /> : null}{personaDocument === undefined ? null : <PersonaEditor option={personas.find((option) => option.id === personaDocument.id) ?? { id: personaDocument.id, displayName: personaDocument.id, builtin: personaDocument.builtin }} document={personaDocument} onSave={onSavePersona} onDelete={onDeletePersona} onRestore={onRestorePersona} onRefresh={onRefreshPersonas} onClose={() => action("closePersona")} />}{state?.personaPickerOpen === true ? <PersonaPicker personas={personas} selectedPersona={form.persona} busy={saving} error={errorFor("companion.persona")} onSelect={(persona) => { void selectPersona(persona); }} onClose={() => action("closePicker")} /> : null}</div>;
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
