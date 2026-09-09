import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";

export function SettingsDiscardDialog({ onCancel, onConfirm }: {
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}): ReactElement {
  const { t } = useI18n();
  return <ConfirmationDialog
    id="settings-discard"
    title={t("settings.discardTitle")}
    description={t("settings.discardDescription")}
    cancelLabel={t("settings.back")}
    confirmLabel={t("settings.discard")}
    onCancel={onCancel}
    onConfirm={onConfirm}
  />;
}
