import type { ReactElement } from "react";

import type { FormState } from "../settings-form.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { QuickActionsEditor } from "./QuickActionsEditor.js";
import { BooleanInput, NumberInput, SelectInput } from "./SettingsControls.js";

export function NotificationSettings({ form, snapshot, update, errorFor }: SettingsCategoryProps): ReactElement {
  return <>
    <fieldset id="settings-notification"><legend>通知と吹き出し</legend>
      <SelectInput label="通知方法" path="notification.mode" value={form.notificationMode} options={snapshot.signedBuild ? ["bubble", "os", "both"] : ["bubble"]} update={(value) => update("notificationMode", value as FormState["notificationMode"])} />
      <label><span>吹き出しの位置</span><select id="setting-bubble-position" value={form.bubblePosition} onChange={(event) => update("bubblePosition", event.target.value as FormState["bubblePosition"])}><option value="bottom-right">右下</option><option value="top-right">右上</option><option value="bottom-left">左下</option><option value="top-left">左上</option></select></label>
      <label><span>表示する画面</span><select id="setting-bubble-display" value={form.bubbleDisplay} onChange={(event) => update("bubbleDisplay", event.target.value as FormState["bubbleDisplay"])}><option value="main">メイン画面</option><option value="cursor">マウスカーソルのある画面</option><option value="front">前面ウィンドウのある画面</option></select></label>
      <BooleanInput label="思考フキダシを表示する" path="ui.thoughtBubble" value={form.thoughtBubble} update={(value) => update("thoughtBubble", value)} />
      <label><span>通常の吹き出し</span><select id="setting-bubble-keepLatest" value={form.bubbleKeepLatest ? "persistent" : "timed"} onChange={(event) => update("bubbleKeepLatest", event.target.value === "persistent")}><option value="timed">一定時間で消す（既定・30秒）</option><option value="persistent">出しっぱなし</option></select></label>
      <p className="field-help">「一定時間で消す」は吹き出しの表示時間（既定30秒）を使います。「出しっぱなし」は最新の通常吹き出しを残します。チュートリアルや一時通知には影響しません。</p>
    </fieldset>
    <fieldset id="settings-popup"><legend>送信ポップアップの定型文</legend>
      <p className="field-help">押すと、この文を添えてすぐ送信します。変更は下の「反映する」で確定します。</p>
      <QuickActionsEditor kind="text" title="テキスト" actions={form.textQuickActions} update={(value) => update("textQuickActions", value)} errorFor={errorFor} />
      <QuickActionsEditor kind="image" title="スクリーンショット" actions={form.imageQuickActions} update={(value) => update("imageQuickActions", value)} errorFor={errorFor} />
    </fieldset>
    <fieldset id="settings-notification-detail"><legend>詳細</legend>
      <SelectInput label="通知する最低 priority" path="notification.minPriority" value={form.minPriority} options={["info", "warning", "critical"]} update={(value) => update("minPriority", value as FormState["minPriority"])} />
      <NumberInput label="吹き出しの表示時間" path="notification.bubbleDurationMs" value={form.bubbleDurationMs} update={(value) => update("bubbleDurationMs", value)} errorFor={errorFor} />
      <NumberInput label="吹き出しの積み上げ数" path="bubble.maxStack" value={form.bubbleMaxStack} update={(value) => update("bubbleMaxStack", value)} errorFor={errorFor} />
      <BooleanInput label="会話に priority を表示" path="notification.showPriority" value={form.showPriority} update={(value) => update("showPriority", value)} />
    </fieldset>
  </>;
}
