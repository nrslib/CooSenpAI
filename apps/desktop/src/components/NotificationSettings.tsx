import type { ReactElement } from "react";

import { t, useI18n, type Locale } from "../i18n/index.js";
import type { FormState } from "../settings-form.js";
import type { AppSnapshot, BubbleDisplaySelection } from "../types.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { QuickActionsEditor } from "./QuickActionsEditor.js";
import { BooleanInput, NumberInput, SelectInput } from "./SettingsControls.js";

interface BubbleDisplayOption {
  readonly value: BubbleDisplaySelection;
  readonly label: string;
}

export function bubbleDisplayOptions(snapshot: AppSnapshot, selected: BubbleDisplaySelection, locale: Locale): readonly BubbleDisplayOption[] {
  const options: BubbleDisplayOption[] = [
    { value: "main", label: t(locale, "settings.notifications.mainScreen") },
    { value: "cursor", label: t(locale, "settings.notifications.cursorScreen") },
    { value: "front", label: t(locale, "settings.notifications.frontScreen") },
  ];
  const nameCounts = new Map<string, number>();
  for (const display of snapshot.displays.items) {
    if (display.name !== "") nameCounts.set(display.name, (nameCounts.get(display.name) ?? 0) + 1);
  }
  const selectedUuid = selected.startsWith("display:") ? selected.slice("display:".length) : null;
  for (const display of snapshot.displays.items) {
    const id = display.uuid.slice(0, 8);
    const name = display.name === "" ? t(locale, "settings.notifications.unnamedDisplay", { id }) : display.name;
    const label = (nameCounts.get(display.name) ?? 0) > 1 ? `${name}（${id}）` : name;
    const value: BubbleDisplaySelection = selectedUuid?.toLowerCase() === display.uuid.toLowerCase() ? selected : `display:${display.uuid}`;
    options.push({ value, label });
  }
  if (selectedUuid !== null && !snapshot.displays.items.some((display) => display.uuid.toLowerCase() === selectedUuid.toLowerCase())) {
    const key = snapshot.displays.loadFailed ? "settings.notifications.unknownDisplay" : "settings.notifications.disconnectedDisplay";
    options.push({ value: selected, label: t(locale, key, { id: selectedUuid.slice(0, 8) }) });
  }
  return options;
}

export function NotificationSettings({ form, snapshot, update, errorFor }: SettingsCategoryProps): ReactElement {
  const { t, locale } = useI18n();
  const displayOptions = bubbleDisplayOptions(snapshot, form.bubbleDisplay, locale);
  return <>
    <fieldset id="settings-notification"><legend>{t("settings.notifications.heading")}</legend>
      <SelectInput label={t("settings.notifications.mode")} path="notification.mode" value={form.notificationMode} options={snapshot.signedBuild ? ["bubble", "os", "both"] : ["bubble"]} update={(value) => update("notificationMode", value as FormState["notificationMode"])} />
      <SettingsSearchItem label={t("settings.notifications.position")} path="bubble.position">
        <label><span>{t("settings.notifications.position")}</span><select id="setting-bubble-position" value={form.bubblePosition} onChange={(event) => update("bubblePosition", event.target.value as FormState["bubblePosition"])}><option value="bottom-right">{t("settings.notifications.bottomRight")}</option><option value="top-right">{t("settings.notifications.topRight")}</option><option value="bottom-left">{t("settings.notifications.bottomLeft")}</option><option value="top-left">{t("settings.notifications.topLeft")}</option></select></label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.notifications.display")} path="bubble.display">
        <label><span>{t("settings.notifications.display")}</span><select id="setting-bubble-display" value={form.bubbleDisplay} onChange={(event) => update("bubbleDisplay", event.target.value as FormState["bubbleDisplay"])}>{displayOptions.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></label>
        {snapshot.displays.loadFailed ? <p className="field-help" role="alert">{t("settings.notifications.displayLoadFailed")}</p> : null}
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.notifications.edgeRecall")} path="bubble.edgeRecall" description={t("settings.notifications.edgeRecallDescription")}>
        <label className="boolean-field"><span>{t("settings.notifications.edgeRecall")}</span><input id="setting-bubble-edgeRecall" type="checkbox" checked={form.bubbleEdgeRecall} onChange={(event) => update("bubbleEdgeRecall", event.target.checked)} /></label>
      </SettingsSearchItem>
      <BooleanInput label={t("settings.notifications.thoughtBubble")} path="ui.thoughtBubble" value={form.thoughtBubble} update={(value) => update("thoughtBubble", value)} />
      <SettingsSearchItem label={t("settings.notifications.normalBubble")} path="bubble.keepLatest" description={t("settings.notifications.normalBubbleDescription")}>
        <label><span>{t("settings.notifications.normalBubble")}</span><select id="setting-bubble-keepLatest" value={form.bubbleKeepLatest ? "persistent" : "timed"} onChange={(event) => update("bubbleKeepLatest", event.target.value === "persistent")}><option value="timed">{t("settings.notifications.timed")}</option><option value="persistent">{t("settings.notifications.persistent")}</option></select></label>
      </SettingsSearchItem>
      <p className="field-help">{t("settings.notifications.normalBubbleHelp")}</p>
    </fieldset>
    <fieldset id="settings-popup"><legend>{t("settings.notifications.popupHeading")}</legend>
      <p className="field-help">{t("settings.notifications.popupHelp")}</p>
      <SettingsSearchItem label={t("settings.notifications.textQuickAction")} path="popup.quickActions.text" description={t("settings.notifications.quickActionDescription")}>
        <QuickActionsEditor kind="text" title={t("capture.text")} actions={form.textQuickActions} update={(value) => update("textQuickActions", value)} errorFor={errorFor} />
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.notifications.screenshotQuickAction")} path="popup.quickActions.image" description={t("settings.notifications.quickActionDescription")}>
        <QuickActionsEditor kind="image" title={t("capture.screen")} actions={form.imageQuickActions} update={(value) => update("imageQuickActions", value)} errorFor={errorFor} />
      </SettingsSearchItem>
    </fieldset>
    <fieldset id="settings-notification-detail"><legend>{t("settings.detail.heading")}</legend>
      <SelectInput label={t("settings.notifications.priority")} path="notification.minPriority" value={form.minPriority} options={["info", "warning", "critical"]} update={(value) => update("minPriority", value as FormState["minPriority"])} />
      <NumberInput label={t("settings.notifications.duration")} path="notification.bubbleDurationMs" value={form.bubbleDurationMs} update={(value) => update("bubbleDurationMs", value)} errorFor={errorFor} />
    </fieldset>
  </>;
}
