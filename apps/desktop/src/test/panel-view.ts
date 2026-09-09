import { vi } from "vitest";
import type { PanelCommand, PanelEvent, PanelKind } from "../usePanelPresenter.js";
import type { IpcResult } from "../types.js";

export const panelView = {
  states: {} as Partial<Record<PanelKind, unknown>>,
  initial: {} as Partial<Record<PanelKind, unknown>>,
  ports: {} as Partial<Record<PanelKind, (command: PanelCommand) => Promise<IpcResult<unknown>>>>,
  inputs: vi.fn<(kind: PanelKind, event: PanelEvent) => void>(),
  reset(): void { this.states = {}; this.initial = {}; this.ports = {}; this.inputs.mockClear(); },
  execute(kind: PanelKind, command: Omit<PanelCommand, "id">): Promise<IpcResult<unknown>> {
    const port = this.ports[kind];
    if (port === undefined) throw new Error(`No port for ${kind}`);
    return port({ id: 1, ...command });
  },
};
const senders = new Map<PanelKind, (event: PanelEvent) => void>();
export function panelViewMock(kind: PanelKind, initial: unknown, execute: (command: PanelCommand) => Promise<IpcResult<unknown>>) {
  panelView.initial[kind] = initial;
  panelView.ports[kind] = execute;
  if (!senders.has(kind)) senders.set(kind, (event) => panelView.inputs(kind, event));
  return { state: panelView.states[kind], error: undefined, send: senders.get(kind)! };
}

export function statusView() {
  return { snapshot: null, error: null, busy: false, visible: true, canCheck: true, canInstall: true, canRestart: true,
    canTest: true, canStop: true, showCheck: false, showInstall: false, showRestart: false, showLater: false };
}
export function settingsView() {
  return { escapeEnabled: true, dirty: false, saving: false, saved: false, closing: false, issues: [], externalChanges: null, discardConfirmOpen: false,
    confirmation: null, activeCategory: "general", recordingShortcut: null, personaDocument: null, personaPickerOpen: false,
    personaPickerAllowed: true, showTutorialPersonaSettings: false, vrmControlsOpen: false };
}
