import type { ReactElement } from "react";

export function CloseIcon(): ReactElement {
  return <svg className="line-icon" viewBox="0 0 24 24" aria-hidden="true">
    <path d="m6.5 6.5 11 11m0-11-11 11" />
  </svg>;
}

export function AddIcon(): ReactElement {
  return <svg className="line-icon" viewBox="0 0 24 24" aria-hidden="true">
    <path d="M12 5v14M5 12h14" />
  </svg>;
}
