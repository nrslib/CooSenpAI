import type { ReactElement } from "react";

import type { FormState } from "../settings-form.js";
import { inputId } from "../settings-form.js";
import { formatShortcutLabel } from "../shortcut-label.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { SelectInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly recordingShortcut?: string;
  readonly setShortcutRecording: (value: string | undefined) => void;
}

export function ShortcutsSettings({ form, snapshot, recordingShortcut, setShortcutRecording, update }: Props): ReactElement {
  return <fieldset id="settings-keyboard"><legend>キーボード</legend>
    <ShortcutInput path="keymap.captureRegion" label="範囲スクリーンショット" value={form.captureShortcut} defaultValue="Alt+Shift+4" recording={recordingShortcut === "capture"} setRecording={(recording) => setShortcutRecording(recording ? "capture" : undefined)} update={(value) => update("captureShortcut", value)} />
    <ShortcutInput path="keymap.sendText" label="テキスト送信" value={form.sendTextShortcut} defaultValue="Alt+Shift+C" recording={recordingShortcut === "text"} setRecording={(recording) => setShortcutRecording(recording ? "text" : undefined)} update={(value) => update("sendTextShortcut", value)} />
    <ShortcutInput path="keymap.copyLastReply" label="直近の返事をコピー" value={form.copyLastReplyShortcut} defaultValue="Alt+Shift+Y" recording={recordingShortcut === "copy-reply"} setRecording={(recording) => setShortcutRecording(recording ? "copy-reply" : undefined)} update={(value) => update("copyLastReplyShortcut", value)} />
    <ShortcutInput path="keymap.microphone" label={`マイク（既定: ${formatShortcutLabel("Alt+Space")}）`} value={form.microphoneShortcut} defaultValue="Alt+Space" recording={recordingShortcut === "microphone"} setRecording={(recording) => setShortcutRecording(recording ? "microphone" : undefined)} update={(value) => update("microphoneShortcut", value)} />
    <ShortcutInput path="keymap.togglePanel" label="パネルの表示 / 非表示" value={form.togglePanelShortcut} defaultValue="Alt+Shift+V" recording={recordingShortcut === "panel"} setRecording={(recording) => setShortcutRecording(recording ? "panel" : undefined)} update={(value) => update("togglePanelShortcut", value)} />
    <ShortcutInput path="keymap.toggleWatch" label="見る / 休憩する" value={form.toggleWatchShortcut} defaultValue="Alt+Shift+W" recording={recordingShortcut === "watch"} setRecording={(recording) => setShortcutRecording(recording ? "watch" : undefined)} update={(value) => update("toggleWatchShortcut", value)} />
    <SelectInput label="送信キー" path="keymap.sendKey" value={form.sendKey} options={["enter", "cmdEnter"]} update={(value) => update("sendKey", value as FormState["sendKey"])} />
    <p className="field-help">欄をクリックしてキーを押します。Esc で取消、Delete で未設定にします。</p>
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
  const displayedValue = value === "" ? "未設定" : formatShortcutLabel(value);
  return <label><span>{label}</span><span className="shortcut-field"><button id={inputId(path)} type="button" disabled={disabled} onClick={() => setRecording(true)} onKeyDown={(event) => {
    if (!recording) return;
    event.preventDefault();
    if (event.key === "Escape") { event.stopPropagation(); setRecording(false); return; }
    if (event.key === "Delete" || event.key === "Backspace") { update(""); setRecording(false); return; }
    const shortcut = shortcutFromKeyboardEvent(event);
    if (shortcut !== undefined) { update(shortcut); setRecording(false); }
  }}>{disabled ? "近日" : recording ? "キーを押してください" : displayedValue}</button><button type="button" disabled={disabled} onClick={() => resetShortcutToDefault(defaultValue, setRecording, update)}>既定に戻す</button></span></label>;
}

export function shortcutFromKeyboardEvent(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">): string | undefined {
  if (["Meta", "Control", "Alt", "Shift"].includes(event.key)) return undefined;
  const modifiers = [event.metaKey ? "CommandOrControl" : "", event.ctrlKey && !event.metaKey ? "Control" : "", event.altKey ? "Alt" : "", event.shiftKey ? "Shift" : ""].filter(Boolean);
  if (modifiers.length === 0) return undefined;
  const key = event.key === " " ? "Space" : event.key.length === 1 ? event.key.toUpperCase() : event.key;
  return [...modifiers, key].join("+");
}
