import type { ReactElement } from "react";
import type { ModelPickerInput, ModelPickerView } from "../model-popup/view.js";
import { useI18n } from "../i18n/index.js";

export function CompanionModelControls({ view, onEvent }: { readonly view: ModelPickerView; readonly onEvent: (event: ModelPickerInput) => void }): ReactElement {
  const { t } = useI18n();
  return <section className="companion-model-controls" aria-label={t("modelControls.label")}>
    <div className="companion-model-control">
      <label htmlFor="companion-provider">Provider</label>
      <select id="companion-provider" value={view.provider} onChange={(event) => onEvent({ type: "provider", value: event.currentTarget.value })} disabled={view.disabled}>
        {view.providers.map((candidate) => <option key={candidate} value={candidate}>{candidate}</option>)}
      </select>
    </div>
    <div className="companion-model-control companion-model-control-model">
      <label htmlFor="companion-model">Model</label>
      <input id="companion-model" value={view.model} list="companion-model-candidates" autoComplete="off" disabled={view.disabled}
        onChange={(event) => onEvent({ type: "model", value: event.currentTarget.value })}
        onBlur={() => onEvent({ type: "commitModel" })}
        onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); onEvent({ type: "commitModel" }); } }} />
      <datalist id="companion-model-candidates">{view.models.map((candidate) => <option key={candidate} value={candidate} />)}</datalist>
    </div>
    <div className="companion-model-control companion-model-control-effort">
      <label htmlFor="companion-effort">Effort</label>
      <input id="companion-effort" value={view.effort} list="companion-effort-candidates" autoComplete="off" disabled={view.disabled}
        onChange={(event) => onEvent({ type: "effort", value: event.currentTarget.value })}
        onBlur={() => onEvent({ type: "commitEffort" })}
        onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); onEvent({ type: "commitEffort" }); } }} />
      <datalist id="companion-effort-candidates">{view.efforts.map((candidate) => <option key={candidate} value={candidate} />)}</datalist>
    </div>
    {view.showClaudeHelp ? <small className="companion-model-help">{t("modelControls.claudeAlias")}</small> : null}
    {view.showReload ? <>
      <button className="companion-model-reload" type="button" onClick={() => onEvent({ type: "reload" })} disabled={view.disabled || view.reloading}>
        {view.reloading ? t("common.reading") : t("common.reload")}
      </button>
      {view.opencodeFailed ? <small className="companion-model-help" role="status">{t("modelControls.opencodeError")}</small> : null}
    </> : null}
    {view.notice ? <small className="companion-model-help" role="status">{t("modelControls.opencodeNotice")}</small> : null}
  </section>;
}
