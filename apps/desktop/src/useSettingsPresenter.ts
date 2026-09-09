import { useEffect } from "react";
import type { AppSnapshot, ConfigIssue, ConfigPatch, CooSenpaiConfig, IpcResult, PersonaDocument } from "./types.js";
import type { SettingsCategory } from "./settings-categories.js";
import type { SettingsDraft, SettingsReflection } from "./settings-form-sync.js";
import { configFields, configResult } from "./settings-form-fields.js";
import { usePanelPresenter, type PanelCommand } from "./usePanelPresenter.js";

export interface SettingsView {
  readonly escapeEnabled: boolean;
  readonly dirty: boolean;
  readonly saving: boolean;
  readonly saved: boolean;
  readonly closing: boolean;
  readonly issues: readonly ConfigIssue[];
  readonly externalChanges: readonly string[] | null;
  readonly discardConfirmOpen: boolean;
  readonly confirmation: "tuning" | "conversation-reset" | null;
  readonly activeCategory: SettingsCategory;
  readonly recordingShortcut: string | null;
  readonly personaDocument: PersonaDocument | null;
  readonly personaPickerOpen: boolean;
  readonly personaPickerAllowed: boolean;
  readonly showTutorialPersonaSettings: boolean;
  readonly vrmControlsOpen: boolean;
}
type SettingsCommand = PanelCommand & (
  | { kind: "save"; payload: { patch: ConfigPatch; avatarImage: readonly number[] | null; baseConfigRevision: number } }
  | { kind: "selectPersona" | "loadPersona" | "focusIssue"; payload: string }
  | { kind: "reflect"; payload: SettingsReflection }
  | { kind: "savedDelay"; payload: number }
  | { kind: "focusFirst" | "resetTuning" | "resetConversation" | "reloadConflict" | "clearPreview" | "close"; payload: null }
);
interface SettingsPort {
  readonly save: (patch: ConfigPatch, image: readonly number[] | undefined, revision: number) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly selectPersona: (id: string) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly getPersona: (id: string) => Promise<IpcResult<PersonaDocument>>;
  readonly reload: () => Promise<IpcResult<CooSenpaiConfig>>;
  readonly resetConversation: () => Promise<IpcResult<unknown>>;
  readonly clearPreview: () => Promise<IpcResult<null>>;
  readonly close: () => void;
  readonly reflect: (value: SettingsReflection) => void;
  readonly resetTuning: () => void;
  readonly focusIssue: (path: string) => void;
  readonly focusFirst: () => void;
}
function observation(snapshot: AppSnapshot, focusSection: "watch" | undefined) {
  return { config: snapshot.config, fields: configFields(snapshot.config), configRevision: snapshot.configRevision, avatarImageLoadFailed: snapshot.avatarImageLoadFailed,
    issues: snapshot.lastError?.kind === "config" ? snapshot.lastError.issues ?? [] : [], onboarding: snapshot.onboarding, focusSection: focusSection ?? null };
}
export function useSettingsPresenter(snapshot: AppSnapshot, focusSection: "watch" | undefined, draft: SettingsDraft, port: SettingsPort) {
  const presenter = usePanelPresenter<SettingsView, SettingsCommand>("settings", { observation: observation(snapshot, focusSection), draft }, async (command) => {
    switch (command.kind) {
      case "save": return configResult(await port.save(command.payload.patch, command.payload.avatarImage ?? undefined, command.payload.baseConfigRevision));
      case "selectPersona": return configResult(await port.selectPersona(command.payload));
      case "loadPersona": return port.getPersona(command.payload);
      case "reloadConflict": return configResult(await port.reload());
      case "resetConversation": return port.resetConversation();
      case "clearPreview": return port.clearPreview();
      case "reflect": port.reflect(command.payload); break;
      case "resetTuning": port.resetTuning(); break;
      case "focusIssue": port.focusIssue(command.payload); break;
      case "focusFirst": port.focusFirst(); break;
      case "close": port.close(); break;
      case "savedDelay": await new Promise<void>((resolve) => window.setTimeout(resolve, command.payload)); break;
    }
    return { ok: true, value: null };
  });
  useEffect(() => presenter.send({ type: "action", name: "snapshot", value: observation(snapshot, focusSection) }),
    [snapshot.config, snapshot.configRevision, snapshot.avatarImageLoadFailed, snapshot.lastError, snapshot.onboarding, focusSection, presenter.send]);
  return presenter;
}
