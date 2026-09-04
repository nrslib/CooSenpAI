import type { ReactElement } from "react";

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

export function VisionSettings({ form, snapshot, advanced, saving, update, errorFor, onOpenSystemSettings, onRelaunch, highlight, onResetTuning }: Props): ReactElement {
  return <>
    <WatchTargets
      fullscreen={form.watchFullscreen}
      apps={form.watchApps}
      highlight={highlight || tutorialSettingsHighlight(snapshot.onboarding, "watch")}
      updateFullscreen={(value) => update("watchFullscreen", value)}
      updateApps={(value) => update("watchApps", value)}
    />
    {advanced ? <fieldset id="settings-watch-detail" disabled={saving}>
      <legend>見るタイミングの詳細</legend>
      <p className="field-help">撮影や区切り検知のタイミングを調整します。各項目には契約上の既定値を表示します。</p>
      <NumberInput label="送信間隔" path="watch.sendIntervalMs" value={form.sendIntervalMs} update={(value) => update("sendIntervalMs", value)} errorFor={errorFor} />
      <NumberInput label="送信の待ち時間" path="watch.sendDebounceMs" value={form.sendDebounceMs} update={(value) => update("sendDebounceMs", value)} errorFor={errorFor} />
      <NumberInput label="1 回のフレーム数" path="watch.framesPerSend" value={form.framesPerSend} update={(value) => update("framesPerSend", value)} errorFor={errorFor} />
      <NumberInput label="送信画像の幅" path="watch.downscaleWidth" value={form.downscaleWidth} update={(value) => update("downscaleWidth", value)} errorFor={errorFor} />
      <NumberInput label="入力停止まで" path="watch.triggers.typingPauseMs" value={form.typingPauseMs} update={(value) => update("typingPauseMs", value)} errorFor={errorFor} />
      <NumberInput label="入力中の判定" path="watch.triggers.activeThresholdMs" value={form.activeThresholdMs} update={(value) => update("activeThresholdMs", value)} errorFor={errorFor} />
      <BooleanInput label="アプリ切り替えを使う" path="watch.triggers.appSwitch" value={form.appSwitch} update={(value) => update("appSwitch", value)} />
      <NumberInput label="アプリ切り替え待ち" path="watch.triggers.appSwitchSettleMs" value={form.appSwitchSettleMs} update={(value) => update("appSwitchSettleMs", value)} errorFor={errorFor} />
      <NumberInput label="最大撮影間隔" path="watch.triggers.maxIntervalMs" value={form.maxIntervalMs} update={(value) => update("maxIntervalMs", value)} errorFor={errorFor} />
      <NumberInput label="撮影の最小間隔" path="watch.triggers.minSpacingMs" value={form.minSpacingMs} update={(value) => update("minSpacingMs", value)} errorFor={errorFor} />
      <NumberInput label="信号の確認間隔" path="watch.triggers.pollMs" value={form.pollMs} update={(value) => update("pollMs", value)} errorFor={errorFor} />
      <BooleanInput label="バッテリー時に間隔を延ばす" path="watch.battery.enabled" value={form.batteryEnabled} update={(value) => update("batteryEnabled", value)} />
      <NumberInput label="バッテリー倍率" path="watch.battery.multiplier" value={form.batteryMultiplier} update={(value) => update("batteryMultiplier", value)} errorFor={errorFor} />
      <BooleanInput label="OCR ゲートを使う" path="watch.ocrGate.enabled" value={form.ocrGateEnabled} update={(value) => update("ocrGateEnabled", value)} />
      <SelectInput label="OCR の精度" path="watch.ocrGate.level" value={form.ocrGateLevel} options={["fast", "accurate"]} update={(value) => update("ocrGateLevel", value as FormState["ocrGateLevel"])} />
      <NumberInput label="OCR のタイムアウト" path="watch.ocrGate.timeoutMs" value={form.ocrGateTimeoutMs} update={(value) => update("ocrGateTimeoutMs", value)} errorFor={errorFor} />
      <TextInput label="OCR helper の実行ファイル（任意）" path="watch.ocrGate.executable" value={form.ocrGateExecutable} update={(value) => update("ocrGateExecutable", value)} />
      <NumberInput label="抜き出す文字の最大文字数" path="observer.textExcerptMaxChars" value={form.observerTextExcerptMaxChars} update={(value) => update("observerTextExcerptMaxChars", value)} errorFor={errorFor} />
      <NumberInput label="抜き出す箇所の最大数" path="observer.textExcerptMaxCount" value={form.observerTextExcerptMaxCount} update={(value) => update("observerTextExcerptMaxCount", value)} errorFor={errorFor} />
      <NumberInput label="文字の総量上限" path="observer.textTotalMaxChars" value={form.observerTextTotalMaxChars} update={(value) => update("observerTextTotalMaxChars", value)} errorFor={errorFor} />
      <NumberInput label="変化の最大数" path="observer.changesMaxCount" value={form.observerChangesMaxCount} update={(value) => update("observerChangesMaxCount", value)} errorFor={errorFor} />
      <NumberInput label="目からの記録をまとめる最大数" path="companion.wakeCoalesceMax" value={form.companionWakeCoalesceMax} update={(value) => update("companionWakeCoalesceMax", value)} errorFor={errorFor} />
      <NumberInput label="session の呼び出し上限" path="companion.sessionMaxCalls" value={form.companionSessionMaxCalls} update={(value) => update("companionSessionMaxCalls", value)} errorFor={errorFor} />
      <NumberInput label="詰まりとみなす時間" path="companion.stuckAfterMs" value={form.companionStuckAfterMs} update={(value) => update("companionStuckAfterMs", value)} errorFor={errorFor} />
      <NumberInput label="配信待ち上限" path="companion.pendingDeliveryLimit" value={form.pendingDeliveryLimit} update={(value) => update("pendingDeliveryLimit", value)} errorFor={errorFor} />
      <NumberInput label="配信待ち容量上限" path="companion.pendingDeliveryMaxBytes" value={form.pendingDeliveryMaxBytes} update={(value) => update("pendingDeliveryMaxBytes", value)} errorFor={errorFor} />
      <NumberInput label="会話後に声をかけない時間（分）" path="companion.proactiveQuietMinutes" value={form.proactiveQuietMinutes} update={(value) => update("proactiveQuietMinutes", value)} errorFor={errorFor} />
      {tuningIsDefault(form) ? null : <button type="button" onClick={onResetTuning}>見るタイミングを既定に戻す</button>}
    </fieldset> : null}
    {advanced ? <fieldset id="settings-retention">
      <legend>保持</legend>
      <NumberInput label="視覚の記録の保存日数" path="retention.observationDays" value={form.observationDays} update={(value) => update("observationDays", value)} errorFor={errorFor} />
      <NumberInput label="会話の保存日数" path="retention.conversationDays" value={form.conversationDays} update={(value) => update("conversationDays", value)} errorFor={errorFor} />
    </fieldset> : null}
    <fieldset id="settings-vision-permissions">
      <legend>権限</legend>
      <p className="field-help">画面収録: {snapshot.screenRecordingMessage ?? permissionLabel(snapshot.screenRecordingStatus)} <button type="button" onClick={onOpenSystemSettings}>設定を開く</button></p>
      {snapshot.screenRecordingRestartRequired ? <div className="button-row"><button type="button" onClick={onRelaunch}>再起動</button></div> : null}
    </fieldset>
  </>;
}
