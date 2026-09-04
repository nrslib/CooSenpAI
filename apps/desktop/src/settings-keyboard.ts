export interface SettingsKeyEvent {
  readonly key: string;
  readonly composing: boolean;
  readonly keyCode: number;
}

export type SettingsEscapeAction = "ignore" | "cancel-recording" | "close";

export function settingsEscapeAction(
  event: SettingsKeyEvent,
  shortcutRecording: boolean,
): SettingsEscapeAction {
  if (event.key !== "Escape" || event.composing || event.keyCode === 229) {
    return "ignore";
  }
  return shortcutRecording ? "cancel-recording" : "close";
}

export function handleSettingsEscape(
  event: SettingsKeyEvent,
  shortcutRecording: boolean,
  close: () => void,
  cancelShortcutRecording: () => void = () => undefined,
): boolean {
  const action = settingsEscapeAction(event, shortcutRecording);
  if (action === "cancel-recording") {
    cancelShortcutRecording();
  } else if (action === "close") {
    close();
  }
  return action !== "ignore";
}

export function shouldCloseSettingsDraft(
  dirty: boolean,
  confirmDiscard: () => boolean,
): boolean {
  return !dirty || confirmDiscard();
}

export function focusFirstSettingsControl(
  root: Pick<HTMLElement, "querySelector">,
): boolean {
  const first = root.querySelector<HTMLElement>(
    "button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])",
  );
  first?.focus();
  return first !== null;
}

export function createSettingsEscapeListener(
  shortcutRecording: () => boolean,
  cancelShortcutRecording: () => void,
  close: () => Promise<unknown>,
): (event: KeyboardEvent) => void {
  return (event) => {
    const action = settingsEscapeAction(
      { key: event.key, composing: event.isComposing, keyCode: event.keyCode },
      shortcutRecording(),
    );
    if (action === "ignore") return;
    event.preventDefault();
    event.stopPropagation();
    if (action === "cancel-recording") cancelShortcutRecording();
    else void close();
  };
}
