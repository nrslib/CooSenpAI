import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { LicenseDocument } from "../types.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";

interface Props extends SettingsCategoryProps {
  readonly onRestartTutorial: () => void;
  readonly onRestartSetup: () => void;
  readonly onResetConversation: () => void;
  readonly onOpenLicenseDocument: (document: LicenseDocument) => void;
}

export function SetupSettings({ snapshot, onRestartTutorial, onRestartSetup, onResetConversation, onOpenLicenseDocument }: Props): ReactElement {
  const { t } = useI18n();
  return <fieldset id="settings-setup"><legend>{t("settings.setup.heading")}</legend>
    <div className="button-row"><button type="button" onClick={onRestartTutorial}>{t("settings.setup.restartTutorial")}</button><button type="button" onClick={onRestartSetup}>{t("settings.setup.restartSetup")}</button></div>
    <button type="button" disabled={snapshot.onboarding.tutorialActive === true} onClick={onResetConversation}>{t("settings.setup.resetConversation")}</button>
    <section id="settings-license" aria-label={t("settings.setup.licenseLabel")}>
      <h3>{t("settings.setup.licenseLabel")}</h3>
      <p>{t("settings.setup.licenseDescription")}</p>
      <div className="button-row"><button type="button" onClick={() => onOpenLicenseDocument("license")}>{t("settings.setup.openLicense")}</button><button type="button" onClick={() => onOpenLicenseDocument("eula")}>{t("settings.setup.openEula")}</button><button type="button" onClick={() => onOpenLicenseDocument("eula-en")}>{t("settings.setup.openEulaEn")}</button></div>
    </section>
  </fieldset>;
}
