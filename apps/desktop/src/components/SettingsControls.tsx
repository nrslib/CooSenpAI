import type { ReactElement } from "react";

import type { CompanionReminder, ProviderName } from "../types.js";
import { useI18n } from "../i18n/index.js";
import { inputId, tuningHelp } from "../settings-form.js";
import { SettingsSearchItem } from "../settings-search.js";

export function NumberInput({ label, path, value, update, errorFor }: {
  readonly label: string;
  readonly path: string;
  readonly value: string;
  readonly update: (value: string) => void;
  readonly errorFor: (path: string) => string | undefined;
}): ReactElement | null {
  const { t } = useI18n();
  const error = errorFor(path);
  const help = tuningHelp[path];
  const helpDescription = help === undefined ? undefined : t(help.descriptionKey);
  const defaultValue = help === undefined ? undefined : help.defaultValueKey === undefined ? help.defaultValue : t(help.defaultValueKey);
  return <SettingsSearchItem label={label} path={path} description={helpDescription}>
    <label>
      <span>{label}{defaultValue === undefined ? "" : t("settings.controls.defaultValue", { value: defaultValue })}</span>
      {helpDescription === undefined ? null : <small>{helpDescription}</small>}
      <input id={inputId(path)} type="number" value={value} onChange={(event) => update(event.target.value)} />
      {error === undefined ? null : <span className="field-error">{error}</span>}
    </label>
  </SettingsSearchItem>;
}

export function TextInput({ label, path, value, update }: {
  readonly label: string;
  readonly path: string;
  readonly value: string;
  readonly update: (value: string) => void;
}): ReactElement | null {
  return <SettingsSearchItem label={label} path={path}>
    <label>{label}<input id={inputId(path)} value={value} onChange={(event) => update(event.target.value)} /></label>
  </SettingsSearchItem>;
}

export function ModelInput({ path, value, options, update }: {
  readonly path: string;
  readonly value: string;
  readonly options: readonly string[];
  readonly update: (value: string) => void;
}): ReactElement | null {
  const { t } = useI18n();
  const listId = `models-${path.replace(/[^a-z0-9-]/giu, "-")}`;
  const label = t("settings.controls.model");
  const description = t("settings.controls.modelDescription");
  return <SettingsSearchItem label={label} path={path} description={description}>
    <label>{label}
      <input id={inputId(path)} list={listId} value={value} onChange={(event) => update(event.target.value)} />
      <datalist id={listId}>{options.map((option) => <option key={option} value={option} />)}</datalist>
      <small>{description}</small>
    </label>
  </SettingsSearchItem>;
}

export function BooleanInput({ label, path, value, update }: {
  readonly label: string;
  readonly path?: string;
  readonly value: boolean;
  readonly update: (value: boolean) => void;
}): ReactElement | null {
  const { t } = useI18n();
  const help = path === undefined ? undefined : tuningHelp[path];
  const helpDescription = help === undefined ? undefined : t(help.descriptionKey);
  const defaultValue = help === undefined ? undefined : help.defaultValueKey === undefined ? help.defaultValue : t(help.defaultValueKey);
  return <SettingsSearchItem label={label} path={path} description={helpDescription}>
    <label className="boolean-field">
      <span>{label}{defaultValue === undefined ? "" : t("settings.controls.defaultValue", { value: defaultValue })}</span>
      {helpDescription === undefined ? null : <small>{helpDescription}</small>}
      <input id={path === undefined ? undefined : inputId(path)} type="checkbox" checked={value} onChange={(event) => update(event.target.checked)} />
    </label>
  </SettingsSearchItem>;
}

export function SelectInput({ label, path, value, options, disabled = false, update }: {
  readonly label: string;
  readonly path?: string;
  readonly value: string;
  readonly options: readonly string[];
  readonly disabled?: boolean;
  readonly update: (value: string) => void;
}): ReactElement | null {
  const { t } = useI18n();
  const help = path === undefined ? undefined : tuningHelp[path];
  const helpDescription = help === undefined ? undefined : t(help.descriptionKey);
  const defaultValue = help === undefined ? undefined : help.defaultValueKey === undefined ? help.defaultValue : t(help.defaultValueKey);
  return <SettingsSearchItem label={label} path={path} description={helpDescription}>
    <label>
      <span>{label}{defaultValue === undefined ? "" : t("settings.controls.defaultValue", { value: defaultValue })}</span>
      {helpDescription === undefined ? null : <small>{helpDescription}</small>}
      <select id={path === undefined ? undefined : inputId(path)} value={value} disabled={disabled} onChange={(event) => update(event.target.value)}>
        {options.map((option) => <option key={option}>{option}</option>)}
      </select>
    </label>
  </SettingsSearchItem>;
}

export function ProviderInput({ path, value, update }: {
  readonly path: string;
  readonly value: ProviderName;
  readonly update: (value: ProviderName) => void;
}): ReactElement | null {
  const { t } = useI18n();
  return <SelectInput label={t("settings.controls.provider")} path={path} value={value} options={["codex", "claude", "opencode"]} update={(next) => update(next as ProviderName)} />;
}

export function ReminderEditor({ value, update }: {
  readonly value: readonly CompanionReminder[];
  readonly update: (value: CompanionReminder[]) => void;
}): ReactElement | null {
  const { t } = useI18n();
  const replace = (index: number, item: CompanionReminder): void => update(value.map((current, currentIndex) => currentIndex === index ? item : current));
  const label = t("settings.controls.reminders");
  return <SettingsSearchItem label={label} path="companion.reminders" description={t("settings.controls.remindersDescription")}>
    <div className="reminder-editor">
      <span>{t("settings.controls.remindersCount")}</span>
      {value.map((item, index) => <div className="reminder-row" key={item.id}>
        <input aria-label={t("settings.controls.reminderTime", { index: index + 1 })} type="time" value={item.time} onChange={(event) => replace(index, { ...item, time: event.target.value })} />
        <input aria-label={t("settings.controls.reminderTheme", { index: index + 1 })} value={item.theme} maxLength={500} placeholder={t("settings.controls.reminderThemePlaceholder")} onChange={(event) => replace(index, { ...item, theme: event.target.value })} />
        <button type="button" onClick={() => update(value.filter((_, currentIndex) => currentIndex !== index))}>{t("common.delete")}</button>
      </div>)}
      <button type="button" disabled={value.length >= 10} onClick={() => update([...value, { id: `reminder-${globalThis.crypto.randomUUID()}`, time: "12:00", theme: t("settings.controls.defaultReminderTheme") }])}>{t("settings.controls.addReminder")}</button>
    </div>
  </SettingsSearchItem>;
}
