import type { ReactElement } from "react";

export type ConfirmationDecision = "cancel" | "confirm";

export function handleConfirmation(
  decision: ConfirmationDecision,
  onCancel: () => void,
  onConfirm: () => void,
): void {
  if (decision === "confirm") onConfirm();
  else onCancel();
}

interface Props {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly cancelLabel: string;
  readonly confirmLabel: string;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}

export function ConfirmationDialog({ id, title, description, cancelLabel, confirmLabel, onCancel, onConfirm }: Props): ReactElement {
  return <div className="confirmation-backdrop" role="presentation"><section className="confirmation-card" role="alertdialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}><h3 id={`${id}-title`}>{title}</h3><p id={`${id}-description`}>{description}</p><div className="button-row"><button type="button" onClick={() => handleConfirmation("cancel", onCancel, onConfirm)}>{cancelLabel}</button><button className="primary" type="button" onClick={() => handleConfirmation("confirm", onCancel, onConfirm)}>{confirmLabel}</button></div></section></div>;
}
