import type { ReactElement } from "react";

import type { CompanionReminder, PersonaOption, ProviderName } from "../types.js";
import { inputId, tuningHelp } from "../settings-form.js";

export function NumberInput({ label, path, value, update, errorFor }: {
  readonly label: string;
  readonly path: string;
  readonly value: string;
  readonly update: (value: string) => void;
  readonly errorFor: (path: string) => string | undefined;
}): ReactElement {
  const error = errorFor(path);
  const help = tuningHelp[path];
  return <label>
    <span>{label}{help === undefined ? "" : `（既定: ${help.defaultValue}）`}</span>
    {help === undefined ? null : <small>{help.description}</small>}
    <input id={inputId(path)} type="number" value={value} onChange={(event) => update(event.target.value)} />
    {error === undefined ? null : <span className="field-error">{error}</span>}
  </label>;
}

export function TextInput({ label, path, value, update }: {
  readonly label: string;
  readonly path: string;
  readonly value: string;
  readonly update: (value: string) => void;
}): ReactElement {
  return <label>{label}<input id={inputId(path)} value={value} onChange={(event) => update(event.target.value)} /></label>;
}

export function ModelInput({ path, value, options, update }: {
  readonly path: string;
  readonly value: string;
  readonly options: readonly string[];
  readonly update: (value: string) => void;
}): ReactElement {
  const listId = `models-${options.join("-").replace(/[^a-z0-9-]/giu, "-")}`;
  return <label>モデル
    <input id={inputId(path)} list={listId} value={value} onChange={(event) => update(event.target.value)} />
    <datalist id={listId}>{options.map((option) => <option key={option} value={option} />)}</datalist>
    <small>候補から選ぶか、モデル名を直接入力できます。</small>
  </label>;
}

export function BooleanInput({ label, path, value, update }: {
  readonly label: string;
  readonly path?: string;
  readonly value: boolean;
  readonly update: (value: boolean) => void;
}): ReactElement {
  const help = path === undefined ? undefined : tuningHelp[path];
  return <label className="boolean-field">
    <span>{label}{help === undefined ? "" : `（既定: ${help.defaultValue}）`}</span>
    {help === undefined ? null : <small>{help.description}</small>}
    <input id={path === undefined ? undefined : inputId(path)} type="checkbox" checked={value} onChange={(event) => update(event.target.checked)} />
  </label>;
}

export function SelectInput({ label, path, value, options, disabled = false, update }: {
  readonly label: string;
  readonly path?: string;
  readonly value: string;
  readonly options: readonly string[];
  readonly disabled?: boolean;
  readonly update: (value: string) => void;
}): ReactElement {
  const help = path === undefined ? undefined : tuningHelp[path];
  return <label>
    <span>{label}{help === undefined ? "" : `（既定: ${help.defaultValue}）`}</span>
    {help === undefined ? null : <small>{help.description}</small>}
    <select id={path === undefined ? undefined : inputId(path)} value={value} disabled={disabled} onChange={(event) => update(event.target.value)}>
      {options.map((option) => <option key={option}>{option}</option>)}
    </select>
  </label>;
}

export function PersonaSelect({ path, value, options, disabled, update }: {
  readonly path: string;
  readonly value: string;
  readonly options: readonly PersonaOption[];
  readonly disabled: boolean;
  readonly update: (value: string) => void;
}): ReactElement {
  return <label>性格<select id={inputId(path)} value={value} disabled={disabled} onChange={(event) => update(event.target.value)}>
    {options.map((option) => <option key={option.id} value={option.id}>{option.builtin ? option.id : `カスタム: ${option.displayName}`}</option>)}
  </select></label>;
}

export function ProviderInput({ path, value, update }: {
  readonly path: string;
  readonly value: ProviderName;
  readonly update: (value: ProviderName) => void;
}): ReactElement {
  return <SelectInput label="プロバイダ" path={path} value={value} options={["codex", "claude", "opencode"]} update={(next) => update(next as ProviderName)} />;
}

export function ReminderEditor({ value, update }: {
  readonly value: readonly CompanionReminder[];
  readonly update: (value: CompanionReminder[]) => void;
}): ReactElement {
  const replace = (index: number, item: CompanionReminder): void => update(value.map((current, currentIndex) => currentIndex === index ? item : current));
  return <div className="reminder-editor">
    <span>時刻を決めた声かけ（最大 10 件）</span>
    {value.map((item, index) => <div className="reminder-row" key={item.id}>
      <input aria-label={`声かけ ${index + 1} の時刻`} type="time" value={item.time} onChange={(event) => replace(index, { ...item, time: event.target.value })} />
      <input aria-label={`声かけ ${index + 1} のテーマ`} value={item.theme} maxLength={500} placeholder="テーマ" onChange={(event) => replace(index, { ...item, theme: event.target.value })} />
      <button type="button" onClick={() => update(value.filter((_, currentIndex) => currentIndex !== index))}>削除</button>
    </div>)}
    <button type="button" disabled={value.length >= 10} onClick={() => update([...value, { id: `reminder-${globalThis.crypto.randomUUID()}`, time: "12:00", theme: "休憩" }])}>声かけを追加</button>
  </div>;
}
