import { useI18n } from "../i18n/index.js";
import { useState, type ReactElement } from "react";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { SettingsSearchItem } from "../settings-search.js";

export function WorkSettings({ form, update, saving, errorFor }: SettingsCategoryProps): ReactElement {
  const { t } = useI18n();
  const [draft, setDraft] = useState("");
  const roots = form.workAllowedRoots;
  const addRoot = (): void => {
    const path = draft.trim();
    if (path === "" || roots.some((root) => root.path === path)) return;
    update("workAllowedRoots", [...roots, { path, read: true, write: false }]);
    setDraft("");
  };
  return <fieldset id="settings-work"><legend>{t("work.category")}</legend>
    <SettingsSearchItem label={t("work.method")} path="work.approvalMode" description={t("work.search")}>
      <label>{t("work.method")}<select id="setting-work-approvalMode" value={form.workApprovalMode} disabled={saving}
        onChange={(event) => update("workApprovalMode", event.target.value as "manual" | "auto")}>
        <option value="manual">{t("work.manual")}</option>
        <option value="auto">{t("work.auto")}</option>
      </select></label>
      <p className="field-help">{t("work.help")}</p>
      {errorFor("work.approvalMode") ? <p role="alert">{errorFor("work.approvalMode")}</p> : null}
    </SettingsSearchItem>
    <SettingsSearchItem label={t("work.roots")} path="work.allowedRoots" description={t("work.rootsSearch")}>
      <p className="field-help">{t("work.rootsHelp")}</p>
      <div className="work-root-list">
        {roots.map((root, index) => <div className="work-root" key={root.path}>
          <code>{root.path}</code>
          <label className="boolean-field"><span>{t("work.rootRead")}</span><input type="checkbox" checked={root.read} disabled={saving}
            aria-label={t("work.rootReadOf", { path: root.path })}
            onChange={(event) => update("workAllowedRoots", roots.map((item) => item.path === root.path ? { ...item, read: event.target.checked } : item))} /></label>
          <label className="boolean-field"><span>{t("work.rootWrite")}</span><input type="checkbox" checked={root.write} disabled={saving}
            aria-label={t("work.rootWriteOf", { path: root.path })}
            onChange={(event) => update("workAllowedRoots", roots.map((item) => item.path === root.path ? { ...item, write: event.target.checked } : item))} /></label>
          <button type="button" disabled={saving} onClick={() => update("workAllowedRoots", roots.filter((item) => item.path !== root.path))}>{t("common.delete")}</button>
          {errorFor(`work.allowedRoots[${index}].path`) ? <p role="alert">{errorFor(`work.allowedRoots[${index}].path`)}</p> : null}
        </div>)}
        {roots.length === 0 ? <p className="muted">{t("work.noRoots")}</p> : null}
      </div>
      <div className="button-row">
        <input id="setting-work-allowedRoots" placeholder={t("work.rootPlaceholder")} value={draft} disabled={saving} onChange={(event) => setDraft(event.target.value)} />
        <button type="button" disabled={saving || draft.trim() === ""} onClick={addRoot}>{t("work.addRoot")}</button>
      </div>
      {errorFor("work.allowedRoots") ? <p role="alert">{errorFor("work.allowedRoots")}</p> : null}
      {errorFor("work") ? <p role="alert">{errorFor("work")}</p> : null}
    </SettingsSearchItem>
    <p className="field-help">{t("work.scope")}</p>
  </fieldset>;
}
