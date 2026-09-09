import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { WatchTargets } from "./WatchTargets.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, NumberInput, SelectInput, TextInput } from "./SettingsControls.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import { permissionLabel, tuningIsDefault } from "../settings-form.js";
import type { FormState } from "../settings-form.js";

interface Props extends SettingsCategoryProps {
  readonly onOpenSystemSettings: () => void;
  readonly onRelaunch: () => void;
  readonly highlight: boolean;
  readonly onResetTuning: () => void;
}

export function VisionSettings({ form, snapshot, saving, update, errorFor, onOpenSystemSettings, onRelaunch, highlight, onResetTuning }: Props): ReactElement {
  const { locale, t } = useI18n();
  return <>
    <WatchTargets
      fullscreen={form.watchFullscreen}
      apps={form.watchApps}
      highlight={highlight || tutorialSettingsHighlight(snapshot.onboarding, "watch")}
      updateFullscreen={(value) => update("watchFullscreen", value)}
      updateApps={(value) => update("watchApps", value)}
    />
    <fieldset id="settings-vision-permissions">
      <legend>{t("settings.vision.permission")}</legend>
      <p className="field-help">{t("settings.vision.screenRecording", { status: snapshot.screenRecordingMessage ?? permissionLabel(snapshot.screenRecordingStatus, locale) })} <button type="button" onClick={onOpenSystemSettings}>{t("common.openSettings")}</button></p>
      {snapshot.screenRecordingRestartRequired ? <div className="button-row"><button type="button" onClick={onRelaunch}>{t("settings.vision.relaunch")}</button></div> : null}
    </fieldset>
    <fieldset id="settings-vision-detail"><legend>{t("settings.vision.detail")}</legend>
      <fieldset id="settings-watch-detail" disabled={saving}>
        <legend>{t("settings.vision.timingDetail")}</legend>
        <p className="field-help">{t("settings.vision.timingHelp")}</p>
        <NumberInput label={t("settings.vision.sendInterval")} path="watch.sendIntervalMs" value={form.sendIntervalMs} update={(value) => update("sendIntervalMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.sendDebounce")} path="watch.sendDebounceMs" value={form.sendDebounceMs} update={(value) => update("sendDebounceMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.framesPerSend")} path="watch.framesPerSend" value={form.framesPerSend} update={(value) => update("framesPerSend", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.downscaleWidth")} path="watch.downscaleWidth" value={form.downscaleWidth} update={(value) => update("downscaleWidth", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.typingPause")} path="watch.triggers.typingPauseMs" value={form.typingPauseMs} update={(value) => update("typingPauseMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.activeThreshold")} path="watch.triggers.activeThresholdMs" value={form.activeThresholdMs} update={(value) => update("activeThresholdMs", value)} errorFor={errorFor} />
        <BooleanInput label={t("settings.vision.appSwitch")} path="watch.triggers.appSwitch" value={form.appSwitch} update={(value) => update("appSwitch", value)} />
        <NumberInput label={t("settings.vision.appSwitchSettle")} path="watch.triggers.appSwitchSettleMs" value={form.appSwitchSettleMs} update={(value) => update("appSwitchSettleMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.maxInterval")} path="watch.triggers.maxIntervalMs" value={form.maxIntervalMs} update={(value) => update("maxIntervalMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.minSpacing")} path="watch.triggers.minSpacingMs" value={form.minSpacingMs} update={(value) => update("minSpacingMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.poll")} path="watch.triggers.pollMs" value={form.pollMs} update={(value) => update("pollMs", value)} errorFor={errorFor} />
        <BooleanInput label={t("settings.vision.batteryEnabled")} path="watch.battery.enabled" value={form.batteryEnabled} update={(value) => update("batteryEnabled", value)} />
        <NumberInput label={t("settings.vision.batteryMultiplier")} path="watch.battery.multiplier" value={form.batteryMultiplier} update={(value) => update("batteryMultiplier", value)} errorFor={errorFor} />
        <BooleanInput label={t("settings.vision.ocrEnabled")} path="watch.ocrGate.enabled" value={form.ocrGateEnabled} update={(value) => update("ocrGateEnabled", value)} />
        <SelectInput label={t("settings.vision.ocrLevel")} path="watch.ocrGate.level" value={form.ocrGateLevel} options={["fast", "accurate"]} update={(value) => update("ocrGateLevel", value as FormState["ocrGateLevel"])} />
        <NumberInput label={t("settings.vision.ocrTimeout")} path="watch.ocrGate.timeoutMs" value={form.ocrGateTimeoutMs} update={(value) => update("ocrGateTimeoutMs", value)} errorFor={errorFor} />
        <TextInput label={t("settings.vision.ocrExecutable")} path="watch.ocrGate.executable" value={form.ocrGateExecutable} update={(value) => update("ocrGateExecutable", value)} />
        <NumberInput label={t("settings.vision.textExcerptMaxChars")} path="observer.textExcerptMaxChars" value={form.observerTextExcerptMaxChars} update={(value) => update("observerTextExcerptMaxChars", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.textExcerptMaxCount")} path="observer.textExcerptMaxCount" value={form.observerTextExcerptMaxCount} update={(value) => update("observerTextExcerptMaxCount", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.textTotalMaxChars")} path="observer.textTotalMaxChars" value={form.observerTextTotalMaxChars} update={(value) => update("observerTextTotalMaxChars", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.changesMaxCount")} path="observer.changesMaxCount" value={form.observerChangesMaxCount} update={(value) => update("observerChangesMaxCount", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.wakeCoalesceMax")} path="companion.wakeCoalesceMax" value={form.companionWakeCoalesceMax} update={(value) => update("companionWakeCoalesceMax", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.sessionMaxCalls")} path="companion.sessionMaxCalls" value={form.companionSessionMaxCalls} update={(value) => update("companionSessionMaxCalls", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.stuckAfterMs")} path="companion.stuckAfterMs" value={form.companionStuckAfterMs} update={(value) => update("companionStuckAfterMs", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.pendingDeliveryLimit")} path="companion.pendingDeliveryLimit" value={form.pendingDeliveryLimit} update={(value) => update("pendingDeliveryLimit", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.pendingDeliveryMaxBytes")} path="companion.pendingDeliveryMaxBytes" value={form.pendingDeliveryMaxBytes} update={(value) => update("pendingDeliveryMaxBytes", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.quietMinutes")} path="companion.proactiveQuietMinutes" value={form.proactiveQuietMinutes} update={(value) => update("proactiveQuietMinutes", value)} errorFor={errorFor} />
        {tuningIsDefault(form) ? null : <button type="button" onClick={onResetTuning}>{t("settings.vision.resetTiming")}</button>}
      </fieldset>
      <fieldset id="settings-retention">
        <legend>{t("settings.vision.retention")}</legend>
        <NumberInput label={t("settings.vision.observationDays")} path="retention.observationDays" value={form.observationDays} update={(value) => update("observationDays", value)} errorFor={errorFor} />
        <NumberInput label={t("settings.vision.conversationDays")} path="retention.conversationDays" value={form.conversationDays} update={(value) => update("conversationDays", value)} errorFor={errorFor} />
      </fieldset>
    </fieldset>
  </>;
}
