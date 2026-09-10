import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { FormState } from "../settings-form.js";
import { inputId } from "../settings-form.js";
import { formatShortcutLabel } from "../shortcut-label.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { SelectInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly recordingShortcut?: string;
  readonly setShortcutRecording: (value: string | undefined) => void;
}

export function ShortcutsSettings({ form, snapshot, recordingShortcut, setShortcutRecording, update }: Props): ReactElement {
  const { t } = useI18n();
  return <fieldset id="settings-keyboard"><legend>{t("settings.shortcuts.heading")}</legend>
    <ShortcutInput path="keymap.captureRegion" label={t("settings.shortcuts.captureRegion")} value={form.captureShortcut} defaultValue="Alt+Shift+4" recording={recordingShortcut === "capture"} setRecording={(recording) => setShortcutRecording(recording ? "capture" : undefined)} update={(value) => update("captureShortcut", value)} />
    <ShortcutInput path="keymap.sendText" label={t("settings.shortcuts.sendText")} value={form.sendTextShortcut} defaultValue="Alt+Shift+C" recording={recordingShortcut === "text"} setRecording={(recording) => setShortcutRecording(recording ? "text" : undefined)} update={(value) => update("sendTextShortcut", value)} />
    <ShortcutInput path="keymap.copyLastReply" label={t("settings.shortcuts.copyLastReply")} value={form.copyLastReplyShortcut} defaultValue="Alt+Shift+Y" recording={recordingShortcut === "copy-reply"} setRecording={(recording) => setShortcutRecording(recording ? "copy-reply" : undefined)} update={(value) => update("copyLastReplyShortcut", value)} />
    <ShortcutInput path="keymap.microphone" label={t("settings.shortcuts.microphone", { shortcut: formatShortcutLabel("Alt+Space") })} value={form.microphoneShortcut} defaultValue="Alt+Space" recording={recordingShortcut === "microphone"} setRecording={(recording) => setShortcutRecording(recording ? "microphone" : undefined)} update={(value) => update("microphoneShortcut", value)} />
    <ShortcutInput path="keymap.togglePanel" label={t("settings.shortcuts.panel")} value={form.togglePanelShortcut} defaultValue="Alt+Shift+V" recording={recordingShortcut === "panel"} setRecording={(recording) => setShortcutRecording(recording ? "panel" : undefined)} update={(value) => update("togglePanelShortcut", value)} />
    <ShortcutInput path="keymap.toggleAvatar" label={t("settings.shortcuts.avatar")} value={form.toggleAvatarShortcut} defaultValue="Alt+Shift+A" recording={recordingShortcut === "avatar"} setRecording={(recording) => setShortcutRecording(recording ? "avatar" : undefined)} update={(value) => update("toggleAvatarShortcut", value)} />
    <ShortcutInput path="keymap.toggleWatch" label={t("settings.shortcuts.watch")} value={form.toggleWatchShortcut} defaultValue="Alt+Shift+W" recording={recordingShortcut === "watch"} setRecording={(recording) => setShortcutRecording(recording ? "watch" : undefined)} update={(value) => update("toggleWatchShortcut", value)} />
    <SelectInput label={t("settings.shortcuts.send")} path="keymap.sendKey" value={form.sendKey} options={["enter", "cmdEnter"]} update={(value) => update("sendKey", value as FormState["sendKey"])} />
    <p className="field-help">{t("settings.shortcuts.help")}</p>
    {snapshot.captureShortcutError === undefined ? null : <p className="field-error">{snapshot.captureShortcutError}</p>}
  </fieldset>;
}

export function resetShortcutToDefault(defaultValue: string, setRecording: (recording: boolean) => void, update: (value: string) => void): void {
  setRecording(false);
  update(defaultValue);
}

function ShortcutInput({ path, label, value, defaultValue, recording, setRecording, update, disabled = false }: {
  readonly path: string;
  readonly label: string;
  readonly value: string;
  readonly defaultValue: string;
  readonly recording: boolean;
  readonly setRecording: (recording: boolean) => void;
  readonly update: (value: string) => void;
  readonly disabled?: boolean;
}): ReactElement {
  const { t } = useI18n();
  const displayedValue = value === "" ? t("settings.shortcuts.unset") : formatShortcutLabel(value);
 return <SettingsSearchItem label={label} path={path}>
   <label><span>{label}</span><span className="shortcut-field"><button id={inputId(path)} type="button" disabled={disabled} onClick={(event) => { event.currentTarget.focus(); setRecording(true); }} onKeyDown={(event) => {
     if (!recording) return;
     event.preventDefault();
     if (event.key === "Escape") { event.stopPropagation(); setRecording(false); return; }
     if ((event.key === "Delete" || event.key === "Backspace") && !(event.metaKey || event.ctrlKey || event.altKey || event.shiftKey)) { update(""); setRecording(false); return; }
     const shortcut = shortcutFromKeyboardEvent(event);
     if (shortcut !== undefined) { update(shortcut); setRecording(false); }
    }}>{disabled ? t("settings.shortcuts.soon") : recording ? t("settings.shortcuts.pressKey") : displayedValue}</button><button type="button" disabled={disabled} onClick={() => resetShortcutToDefault(defaultValue, setRecording, update)}>{t("settings.shortcuts.reset")}</button></span></label>
 </SettingsSearchItem>;
}

export function shortcutFromKeyboardEvent(event: Pick<KeyboardEvent, "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">): string | undefined {
  if (["Meta", "Control", "Alt", "Shift"].includes(event.key)) return undefined;
  if (event.code === "" || event.code === "Unidentified") return undefined;
  if (!supportedShortcutCodes.has(event.code)) return undefined;
  const modifiers = [event.metaKey ? "CommandOrControl" : "", event.ctrlKey ? "Control" : "", event.altKey ? "Alt" : "", event.shiftKey ? "Shift" : ""].filter(Boolean);
  if (modifiers.length === 0) return undefined;
  const key = /^(Key[A-Z]|Digit[0-9])$/.test(event.code) ? event.code.replace(/^(Key|Digit)/, "") : event.code;
  return [...modifiers, key].join("+");
}

const supportedShortcutCodes = new Set([
  ...Array.from({ length: 26 }, (_, index) => `Key${String.fromCharCode(65 + index)}`),
  ...Array.from({ length: 10 }, (_, index) => `Digit${index}`),
  "Equal", "Minus", "BracketLeft", "BracketRight", "Quote", "Semicolon", "Backslash", "Comma", "Slash", "Period", "Backquote",
  "Enter", "Tab", "Space", "Backspace", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20",
  "NumpadDecimal", "NumpadMultiply", "NumpadAdd", "NumLock", "NumpadDivide", "NumpadEnter", "NumpadSubtract", "NumpadEqual",
  ...Array.from({ length: 10 }, (_, index) => `Numpad${index}`),
  "AudioVolumeUp", "AudioVolumeDown", "AudioVolumeMute", "MediaPlayPause", "MediaTrackNext", "MediaTrackPrevious", "Insert", "Home", "PageUp", "Delete", "End", "PageDown", "ArrowLeft", "ArrowRight", "ArrowDown", "ArrowUp", "CapsLock", "PrintScreen",
]);
