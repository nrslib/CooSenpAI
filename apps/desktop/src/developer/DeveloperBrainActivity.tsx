import { useState, type ReactElement } from "react";
import { useI18n } from "../i18n/index.js";
import { desktopApi } from "../ipc.js";

export function DeveloperBrainActivity(): ReactElement {
  const { t } = useI18n();
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const open = async (): Promise<void> => {
    setOpening(true); setError(null);
    const result = await desktopApi.openBrainActivity();
    if (!result.ok) setError(result.error.message);
    setOpening(false);
  };
  return <section aria-labelledby="settings-brain-heading">
    <h3 id="settings-brain-heading">{t("brain.title")}</h3>
    <p className="field-help">{t("brain.developerDescription")}</p>
    <button id="settings-brain-open" type="button" disabled={opening} onClick={() => { void open(); }}>{t("brain.open")}</button>
    {error === null ? null : <p role="alert">{error}</p>}
  </section>;
}
