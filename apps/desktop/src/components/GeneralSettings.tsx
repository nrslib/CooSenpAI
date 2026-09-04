import type { ChangeEvent, RefObject, ReactElement } from "react";

import type { PersonaOption } from "../types.js";
import { MemoryPanel } from "../MemoryPanel.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import { fontPreset } from "../settings-form.js";
import type { FormState } from "../settings-form.js";
import { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, NumberInput, PersonaSelect, ReminderEditor, SelectInput, TextInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly personas: readonly PersonaOption[];
  readonly avatarInputRef: RefObject<HTMLInputElement | null>;
  readonly onSelectAvatar: (event: ChangeEvent<HTMLInputElement>) => void;
  readonly onResetAvatar: () => void;
  readonly onReloadPersona: () => void;
  readonly onEditPersona: () => void;
}

export function GeneralSettings({ form, snapshot, advanced, saving, update, errorFor, personas, avatarInputRef, onSelectAvatar, onResetAvatar, onReloadPersona, onEditPersona }: Props): ReactElement {
  return <>
    <fieldset id="settings-companion" className={tutorialSettingsHighlight(snapshot.onboarding, "persona") ? "tutorial-highlight" : undefined}>
      <legend>性格</legend>
      <p className="field-help">名前と、話し方を決める性格は別々に変更できます。</p>
      <label><span>呼び名（既定: Coo）</span><input id="setting-companion-displayName" value={form.displayName} maxLength={20} disabled={saving} onChange={(event) => update("displayName", event.target.value)} />{errorFor("companion.displayName") === undefined ? null : <span className="field-error">{errorFor("companion.displayName")}</span>}</label>
      <label><span>アバターの色</span><input id="setting-ui-avatarColor" type="color" value={form.avatarColor} onChange={(event) => update("avatarColor", event.target.value)} /></label>
      <label><span>アバター画像</span><input ref={avatarInputRef} id="setting-ui-avatarPath" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" disabled={saving} onChange={onSelectAvatar} /><small className={form.avatarImageLoadFailed ? "field-error" : undefined}>{form.avatarImageLoadFailed ? "設定された画像を読み込めないため、既定のアバターを使用中" : form.avatarFileName === undefined ? form.avatarPath === null ? "未設定（既定のアバターを使用）" : "設定済みのアバターを使用中" : `選択中: ${form.avatarFileName}（反映するまで未保存）`}</small></label>
      <div className="button-row"><button type="button" disabled={saving || (form.avatarPath === null && form.avatarImage === undefined)} onClick={onResetAvatar}>既定のアバターに戻す</button></div>
      <PersonaSelect path="companion.persona" value={form.persona} options={personas} disabled={saving} update={(value) => update("persona", value)} />
      <div className="button-row"><button type="button" disabled={saving} onClick={onReloadPersona}>性格を再読込</button><button type="button" onClick={onEditPersona}>性格を編集</button></div>
      <SelectInput label="積極性" path="companion.assertiveness" value={form.assertiveness} options={["low", "normal", "high"]} update={(value) => update("assertiveness", value as FormState["assertiveness"])} />
      <p className="field-help">ふだんの基準です。メイン画面のチップで一時的に変えられます。</p>
      {advanced ? <>
        <label><span>今日のふりかえり時刻</span><input id="setting-companion-reviewTime" type="time" value={form.reviewTime} onChange={(event) => update("reviewTime", event.target.value)} /><small>空欄にすると無効です。</small></label>
        <ReminderEditor value={form.reminders} update={(value) => update("reminders", value)} />
        <label><span>考え中の送信（既定: 順番待ち）</span><select id="setting-chat-whileThinking" value={form.whileThinking} onChange={(event) => update("whileThinking", event.target.value as FormState["whileThinking"])}><option value="queue">順番待ち</option><option value="append">言い足し</option></select><small>言い足しでは、考え中に送った発言も含めて一つの返事にまとめます。</small></label>
      </> : null}
    </fieldset>

    <fieldset id="settings-appearance"><legend>見た目</legend>
      <label><span>テーマ</span><select id="setting-ui-theme" value={form.uiTheme} onChange={(event) => update("uiTheme", event.target.value as FormState["uiTheme"])}><option value="system">自動</option><option value="light">ライト</option><option value="dark">ダーク</option></select></label>
      <label><span>フォント</span><select id="setting-ui-font" value={fontPreset(form.uiFont)} onChange={(event) => { if (event.target.value !== "custom") update("uiFont", event.target.value); }}><option value="system">システム</option><option value="rounded">丸ゴシック</option><option value="serif">明朝</option><option value="mono">等幅</option><option value="custom">インストール済みフォント</option></select></label>
      <label><span>フォント名（自由入力）</span><input value={fontPreset(form.uiFont) === "custom" ? form.uiFont : ""} placeholder="例: A-OTF UD新ゴ Pr6N" onChange={(event) => update("uiFont", event.target.value)} /></label>
    </fieldset>

    <fieldset id="settings-language"><legend>言語</legend>
      {advanced ? <TextInput label="認識ロケール（既定: system）" path="speech.locale" value={form.speechLocale} update={(value) => update("speechLocale", value)} /> : <p className="field-help">音声認識・文字起こしの言語は「詳細」で変更できます。</p>}
    </fieldset>

    <fieldset id="settings-memory"><legend>記憶</legend>
      <BooleanInput label="記憶を有効にする" path="memory.enabled" value={form.memoryEnabled} update={(value) => update("memoryEnabled", value)} />
      <BooleanInput label="昨日までの会話と観察の要約を AI に渡す" path="memory.providerConsent" value={form.memoryProviderConsent} update={(value) => update("memoryProviderConsent", value)} />
      <p className="field-help">前日の記録から要約を作り、以後の会話に添付します。</p>
      {advanced ? <>
        <NumberInput label="日付変更後の待ち時間（分）" path="memory.graceMinutes" value={form.memoryGraceMinutes} update={(value) => update("memoryGraceMinutes", value)} errorFor={errorFor} />
        <NumberInput label="日次要約の保持日数" path="memory.dailyRetentionDays" value={form.memoryDailyRetentionDays} update={(value) => update("memoryDailyRetentionDays", value)} errorFor={errorFor} />
        <NumberInput label="週次要約の保持週数" path="memory.weeklyRetentionWeeks" value={form.memoryWeeklyRetentionWeeks} update={(value) => update("memoryWeeklyRetentionWeeks", value)} errorFor={errorFor} />
        <NumberInput label="文脈の再注入間隔" path="companion.contextRefreshCalls" value={form.contextRefreshCalls} update={(value) => update("contextRefreshCalls", value)} errorFor={errorFor} />
        <NumberInput label="覚える候補を聞く1日の上限" path="memory.factPromptDailyLimit" value={form.factPromptDailyLimit} update={(value) => update("factPromptDailyLimit", value)} errorFor={errorFor} />
      </> : null}
    </fieldset>
    <MemoryPanel status={snapshot.memoryStatus} />

    {advanced ? <fieldset id="settings-debug"><legend>デバッグ</legend><BooleanInput label="デバッグ記録を残す" path="debug.enabled" value={form.debugEnabled} update={(value) => update("debugEnabled", value)} /><p className="field-help">有効時は送信画像、OCR、プロンプト、AI 応答を最大 3 日・200 MiB 保存します。</p></fieldset> : null}
    <fieldset id="settings-app"><legend>アプリ</legend><BooleanInput label="更新の確認" path="app.checkForUpdates" value={form.checkForUpdates} update={(value) => update("checkForUpdates", value)} /><p className="field-help">GitHub へ版の確認だけを送ります。</p><BooleanInput label="ログイン時に起動する" path="app.launchAtLogin" value={form.launchAtLogin} update={(value) => update("launchAtLogin", value)} /></fieldset>
  </>;
}
