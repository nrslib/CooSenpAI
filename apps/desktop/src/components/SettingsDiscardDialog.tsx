import type { ReactElement } from "react";

import { ConfirmationDialog } from "./ConfirmationDialog.js";

export function SettingsDiscardDialog({ onCancel, onConfirm }: {
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}): ReactElement {
  return <ConfirmationDialog
    id="settings-discard"
    title="変更を破棄しますか？"
    description="反映していない変更は元に戻ります。"
    cancelLabel="設定に戻る"
    confirmLabel="破棄して閉じる"
    onCancel={onCancel}
    onConfirm={onConfirm}
  />;
}
