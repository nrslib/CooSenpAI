import { useEffect, useState, type ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { desktopApi, ipcTransportError } from "../ipc.js";
import { inputId } from "../settings-form.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { VoiceOutputVoice } from "../voice-output.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { VoiceOutputPanel } from "./VoiceOutputPanel.js";

export function VoiceOutputSettings({ form, snapshot, saving, update, errorFor }: SettingsCategoryProps): ReactElement {
  const { t } = useI18n();
  const [voices, setVoices] = useState<readonly VoiceOutputVoice[]>();
  const [loading, setLoading] = useState(false);
  const [voicesError, setVoicesError] = useState<string>();
  const [linkError, setLinkError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [pendingCharacter, setPendingCharacter] = useState<string>();

  useEffect(() => {
    if (form.voiceOutputProvider !== "voicevox") return;
    let active = true;
    setLoading(true);
    setVoices(undefined);
    setVoicesError(undefined);
    void desktopApi.getVoiceOutputVoices().then((result) => {
      if (!active) return;
      if (result.ok) setVoices(result.value);
      else setVoicesError(result.error.message);
    }).catch((cause: unknown) => {
      if (active) setVoicesError(ipcTransportError(cause));
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [form.voiceOutputProvider, reload]);

  const selectedVoice = voices?.find((voice) => voice.id === form.voicevoxStyleId);
  const character = selectedVoice?.name ?? pendingCharacter ?? "";
  const characters = [...new Set(voices?.map((voice) => voice.name) ?? [])];
  const styles = voices?.filter((voice) => voice.name === character) ?? [];
  const openLink = async (kind: "download" | "terms"): Promise<void> => {
    setLinkError(undefined);
    const result = await desktopApi.openVoiceOutputLink(kind);
    if (!result.ok) setLinkError(result.error.message);
  };
  return <>
    <fieldset id="settings-voice-output"><legend>{t("settings.voiceOutput.heading")}</legend>
      <SettingsSearchItem label={t("settings.voiceOutput.enabled")} path="voiceOutput.enabled" description={t("settings.voiceOutput.enabledDescription")}>
        <label className="boolean-field"><span>{t("settings.voiceOutput.enabledLabel")}</span><input id={inputId("voiceOutput.enabled")} type="checkbox" checked={form.voiceOutputEnabled} disabled={saving} onChange={(event) => update("voiceOutputEnabled", event.target.checked)} />{errorFor("voiceOutput.enabled") === undefined ? null : <span className="field-error">{errorFor("voiceOutput.enabled")}</span>}</label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.voiceOutput.provider")} path="voiceOutput.provider" description={t("settings.voiceOutput.providerDescription")}>
        <label><span>{t("settings.voiceOutput.provider")}</span><select id={inputId("voiceOutput.provider")} value={form.voiceOutputProvider} disabled={saving} onChange={(event) => update("voiceOutputProvider", event.target.value as typeof form.voiceOutputProvider)}><option value="system">{t("settings.voiceOutput.providerName")}</option><option value="voicevox">VOICEVOX</option></select>{errorFor("voiceOutput.provider") === undefined ? null : <span className="field-error">{errorFor("voiceOutput.provider")}</span>}</label>
        <p className="field-help">{t("settings.voiceOutput.voicevoxHelp")}</p>
        <div className="button-row"><button type="button" onClick={() => { void openLink("download"); }}>{t("settings.voiceOutput.voicevoxDownload")}</button><button type="button" onClick={() => { void openLink("terms"); }}>{t("settings.voiceOutput.voicevoxTerms")}</button></div>
        {linkError === undefined ? null : <p role="alert" className="field-error">{linkError}</p>}
      </SettingsSearchItem>
      {form.voiceOutputProvider === "voicevox" && <SettingsSearchItem label={t("settings.voiceOutput.voicevoxVoices")} path="voiceOutput.voicevoxStyleId" description={t("settings.voiceOutput.voicevoxVoicesDescription")}>
        <label><span>{t("settings.voiceOutput.voicevoxCharacter")}</span><select value={character} disabled={saving || loading || voices === undefined || voices.length === 0} onChange={(event) => {
          const name = event.target.value;
          setPendingCharacter(name);
          const available = voices?.filter((voice) => voice.name === name) ?? [];
          update("voicevoxStyleId", available.length === 1 ? available[0]!.id : null);
        }}><option value="">{t("settings.voiceOutput.voicevoxCharacterPlaceholder")}</option>{characters.map((name) => <option key={name} value={name}>{name}</option>)}</select></label>
        <label><span>{t("settings.voiceOutput.voicevoxStyle")}</span><select id={inputId("voiceOutput.voicevoxStyleId")} value={form.voicevoxStyleId === null ? "" : String(form.voicevoxStyleId)} disabled={saving || loading || voices === undefined || voices.length === 0} onChange={(event) => update("voicevoxStyleId", event.target.value === "" ? null : Number(event.target.value))}>
          <option value="">{t("settings.voiceOutput.voicevoxStylePlaceholder")}</option>
          {form.voicevoxStyleId !== null && selectedVoice === undefined && <option value={form.voicevoxStyleId} disabled>{t("settings.voiceOutput.voicevoxSavedVoice", { id: form.voicevoxStyleId })}</option>}
          {styles.map((voice) => <option key={voice.id} value={voice.id}>{voice.name} / {voice.styleName}</option>)}
        </select>{errorFor("voiceOutput.voicevoxStyleId") === undefined ? null : <span className="field-error">{errorFor("voiceOutput.voicevoxStyleId")}</span>}</label>
        {selectedVoice !== undefined && <p>{t("settings.voiceOutput.voicevoxCredit", { name: selectedVoice.name })}</p>}
        {loading && <p role="status">{t("settings.voiceOutput.voicevoxLoading")}</p>}
        {voicesError !== undefined && <p role="alert" className="field-error">{t("settings.voiceOutput.voicevoxLoadError")} {voicesError}</p>}
        {voices?.length === 0 && <p role="status">{t("settings.voiceOutput.voicevoxEmpty")}</p>}
        {voices !== undefined && voices.length > 0 && form.voicevoxStyleId !== null && selectedVoice === undefined && <p role="alert" className="field-error">{t("settings.voiceOutput.voicevoxMissing")}</p>}
        <button type="button" disabled={loading || saving} onClick={() => setReload((current) => current + 1)}>{t("settings.voiceOutput.voicevoxRefresh")}</button>
      </SettingsSearchItem>}
      <SettingsSearchItem label={t("settings.voiceOutput.rate")} path="voiceOutput.rate" description={t("settings.voiceOutput.rateDescription")}>
        <label><span>{t("settings.voiceOutput.rateLabel")}</span><small>{t("settings.voiceOutput.rateDescription")}</small><input id={inputId("voiceOutput.rate")} type="number" min={100} max={400} step={1} value={form.voiceOutputRate} disabled={saving} onChange={(event) => update("voiceOutputRate", event.target.value)} />{errorFor("voiceOutput.rate") === undefined ? null : <span className="field-error">{errorFor("voiceOutput.rate")}</span>}</label>
      </SettingsSearchItem>
    </fieldset>
    <SettingsSearchItem label={t("settings.voiceOutput.test")} path="voiceOutput" description={t("settings.voiceOutput.testDescription")}>
      <VoiceOutputPanel enabled={snapshot.config.voiceOutput.enabled} showTest />
    </SettingsSearchItem>
  </>;
}
