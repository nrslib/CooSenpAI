import { useEffect } from "react";
import type { AppSnapshot, ConfigIssue, ConfigPatch, CooSenpaiConfig, DebugWakeResult, IpcResult, MergedSpeakers, PersonaDocument, SpeakerDirectory, SpeakerManagementPayload, SpeakerRenamePayload, SpeakerSummary } from "./types.js";
import type { SettingsCategory } from "./settings-categories.js";
import type { SettingsResetRequest } from "./settings-form.js";
import type { SettingsResetFields } from "./settings-form-fields.js";
import type { SettingsDraft, SettingsReflection } from "./settings-form-sync.js";
import { configFields, configResult } from "./settings-form-fields.js";
import { usePanelPresenter, type PanelCommand } from "./usePanelPresenter.js";

export interface SpeakerManagementView {
  readonly loading: boolean;
  readonly directoryError: string | null;
  readonly speakers: readonly SpeakerSummary[] | null;
  readonly merged: readonly MergedSpeakers[] | null;
  readonly busy: boolean;
  readonly succeeded: boolean;
  readonly error: string | null;
  readonly deleteTarget: string | null;
  readonly controls: {
    readonly draft: Readonly<Record<string, string>>;
    readonly mergeAvailable: boolean; readonly undoAvailable: boolean;
    readonly canMerge: boolean; readonly canUndo: boolean; readonly canRename: boolean;
    readonly canClear: boolean; readonly canEditName: boolean;
    readonly canReregister: boolean; readonly canDelete: boolean;
  };
}

export interface DebugWakeView {
  readonly phase: "idle" | "loading" | "ready" | "processing" | "emitted" | "silent" | "deferred" | "error";
  readonly selectedName: string | null;
  readonly context: string;
  readonly result: DebugWakeResult | null;
  readonly error: string | null;
}

export interface SettingsView {
  readonly feedbackExport: { readonly busy: boolean; readonly path: string | null; readonly error: string | null };
  readonly escapeEnabled: boolean;
  readonly dirty: boolean;
  readonly saving: boolean;
  readonly saved: boolean;
  readonly closing: boolean;
  readonly issues: readonly ConfigIssue[];
  readonly externalChanges: readonly string[] | null;
  readonly discardConfirmOpen: boolean;
  readonly confirmation: "conversation-reset" | "settings-reset" | "speaker-delete" | "speaker-delete-all" | null;
  readonly resetScope: "page" | "all" | null;
  readonly resetCategory: SettingsCategory | null;
  readonly canUndoReset: boolean;
  readonly defaultConfig?: CooSenpaiConfig;
  readonly activeCategory: SettingsCategory;
  readonly recordingShortcut: string | null;
  readonly personaDocument: PersonaDocument | null;
  readonly personaPickerOpen: boolean;
  readonly personaPickerAllowed: boolean;
  readonly showTutorialPersonaSettings: boolean;
  readonly vrmControlsOpen: boolean;
  readonly speakerManagement: SpeakerManagementView;
  readonly debugWake: DebugWakeView;
}
type SettingsCommand = PanelCommand & (
  | { kind: "save"; payload: { patch: ConfigPatch; avatarImage: readonly number[] | null; baseConfigRevision: number } }
  | { kind: "selectPersona" | "loadPersona" | "focusIssue"; payload: string }
  | { kind: "reflect"; payload: SettingsReflection }
  | { kind: "savedDelay"; payload: number }
  | { kind: "resetDraft"; payload: { readonly fields: SettingsResetFields; readonly scope: SettingsResetRequest["scope"]; readonly category: SettingsCategory | null; readonly defaults: CooSenpaiConfig } }
  | { kind: "restoreReset"; payload: { readonly fields: SettingsResetFields; readonly scope: SettingsResetRequest["scope"]; readonly category: SettingsCategory | null; readonly defaults: CooSenpaiConfig } }
  | { kind: "speakerManagement"; payload: SpeakerManagementPayload }
  | { kind: "speakerRename"; payload: SpeakerRenamePayload }
  | { kind: "debugWake"; payload: { readonly image: readonly number[]; readonly context: string } }
  | { kind: "focusFirst" | "resetConversation" | "reloadConflict" | "clearPreview" | "close" | "speakerDirectory" | "feedbackExport"; payload: null }
);
interface SettingsPort {
  readonly feedbackExport: () => Promise<IpcResult<string>>;
  readonly save: (patch: ConfigPatch, image: readonly number[] | undefined, revision: number) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly selectPersona: (id: string) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly getPersona: (id: string) => Promise<IpcResult<PersonaDocument>>;
  readonly reload: () => Promise<IpcResult<CooSenpaiConfig>>;
  readonly resetConversation: () => Promise<IpcResult<unknown>>;
  readonly clearPreview: () => Promise<IpcResult<null>>;
  readonly speakerManagement: (payload: SpeakerManagementPayload) => Promise<IpcResult<null>>;
  readonly speakerDirectory: () => Promise<IpcResult<SpeakerDirectory>>;
  readonly speakerRename: (payload: SpeakerRenamePayload) => Promise<IpcResult<SpeakerDirectory>>;
  readonly debugWake: (image: readonly number[], context: string) => Promise<IpcResult<DebugWakeResult>>;
  readonly close: () => void;
  readonly reflect: (value: SettingsReflection) => void;
  readonly resetDraft: (payload: Extract<SettingsCommand, { kind: "resetDraft" }>['payload']) => void;
  readonly restoreReset: (payload: Extract<SettingsCommand, { kind: "restoreReset" }>['payload']) => void;
  readonly focusIssue: (path: string) => void;
  readonly focusFirst: () => void;
}
function observation(snapshot: AppSnapshot, focusSection: "watch" | "providers" | undefined, focusGeneration: number) {
  return { speakerDirectoryRevision: snapshot.speakerDirectoryRevision ?? 0, config: snapshot.config, fields: configFields(snapshot.config), configRevision: snapshot.configRevision, avatarImageLoadFailed: snapshot.avatarImageLoadFailed,
    issues: snapshot.lastError?.kind === "config" ? snapshot.lastError.issues ?? [] : [], onboarding: snapshot.onboarding, focusSection: focusSection ?? null, focusGeneration };
}
export function useSettingsPresenter(snapshot: AppSnapshot, focusSection: "watch" | "providers" | undefined, focusGeneration: number, draft: SettingsDraft, port: SettingsPort) {
  const presenter = usePanelPresenter<SettingsView, SettingsCommand>("settings", { observation: observation(snapshot, focusSection, focusGeneration), draft }, async (command) => {
    switch (command.kind) {
      case "save": return configResult(await port.save(command.payload.patch, command.payload.avatarImage ?? undefined, command.payload.baseConfigRevision));
      case "selectPersona": return configResult(await port.selectPersona(command.payload));
      case "loadPersona": return port.getPersona(command.payload);
      case "reloadConflict": return configResult(await port.reload());
      case "resetConversation": return port.resetConversation();
      case "clearPreview": return port.clearPreview();
      case "speakerManagement": return port.speakerManagement(command.payload);
      case "feedbackExport": return port.feedbackExport();
      case "speakerDirectory": return port.speakerDirectory();
      case "speakerRename": return port.speakerRename(command.payload);
      case "debugWake": return port.debugWake(command.payload.image, command.payload.context);
      case "reflect": port.reflect(command.payload); break;
      case "resetDraft": port.resetDraft(command.payload); break;
      case "restoreReset": port.restoreReset(command.payload); break;
      case "focusIssue": port.focusIssue(command.payload); break;
      case "focusFirst": port.focusFirst(); break;
      case "close": port.close(); break;
      case "savedDelay": await new Promise<void>((resolve) => window.setTimeout(resolve, command.payload)); break;
    }
    return { ok: true, value: null };
  });
  useEffect(() => presenter.send({ type: "action", name: "snapshot", value: observation(snapshot, focusSection, focusGeneration) }),
    [snapshot.speakerDirectoryRevision, snapshot.config, snapshot.configRevision, snapshot.avatarImageLoadFailed, snapshot.lastError, snapshot.onboarding, focusSection, focusGeneration, presenter.send]);
  return presenter;
}
