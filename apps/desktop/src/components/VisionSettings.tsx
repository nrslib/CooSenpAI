import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { WatchTargets } from "./WatchTargets.js";
import { CaptureAllowlist } from "./CaptureAllowlist.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import type { AppPickerView } from "../useSettingsPresenter.js";
import { BooleanInput } from "./SettingsControls.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import { permissionLabel } from "../settings-form.js";

interface Props extends SettingsCategoryProps {
  readonly appPicker: AppPickerView;
  readonly onAppPickerAction: (name: string, value: unknown) => void;
  readonly onOpenSystemSettings: () => void;
  readonly onOpenAccessibilitySettings?: () => void;
  readonly onRelaunch: () => void;
  readonly highlight: boolean;
}

export function VisionSettings({ form, snapshot, update, appPicker, onAppPickerAction, onOpenSystemSettings, onOpenAccessibilitySettings, onRelaunch, highlight }: Props): ReactElement {
  const { locale, t } = useI18n();
  return <>
    <WatchTargets
      fullscreen={form.watchFullscreen}
      apps={form.watchApps}
      highlight={highlight || tutorialSettingsHighlight(snapshot.onboarding, "watch")}
      updateFullscreen={(value) => update("watchFullscreen", value)}
      updateApps={(value) => update("watchApps", value)}
      picker={appPicker}
      onPickerAction={onAppPickerAction}
    />
    <CaptureAllowlist
      enabled={form.captureAllowlistEnabled}
      apps={form.captureAllowlistApps}
      osMajorVersion={snapshot.osVersion.major}
      updateEnabled={(value) => update("captureAllowlistEnabled", value)}
      updateApps={(value) => update("captureAllowlistApps", value)}
      picker={appPicker}
      onPickerAction={onAppPickerAction}
    />
    <fieldset id="settings-vision-permissions">
      <legend>{t("settings.vision.permission")}</legend>
      <p className="field-help">{t("settings.vision.screenRecording", { status: snapshot.screenRecordingMessage ?? permissionLabel(snapshot.screenRecordingStatus, locale) })} <button type="button" onClick={onOpenSystemSettings}>{t("common.openSettings")}</button></p>
      <p className="field-help">{t("settings.vision.focusElementHelp")} {onOpenAccessibilitySettings === undefined ? null : <button type="button" onClick={onOpenAccessibilitySettings}>{t("common.openSettings")}</button>}</p>
      {snapshot.screenRecordingRestartRequired ? <div className="button-row"><button type="button" onClick={onRelaunch}>{t("settings.vision.relaunch")}</button></div> : null}
    </fieldset>
    <fieldset id="settings-vision-detail">
      <legend>{t("settings.vision.detail")}</legend>
      <BooleanInput label={t("settings.vision.focusElement")} path="watch.focusElement" value={form.watchFocusElement} update={(value) => update("watchFocusElement", value)} />
    </fieldset>
  </>;
}
