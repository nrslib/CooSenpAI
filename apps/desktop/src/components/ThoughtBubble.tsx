import type { ReactElement } from "react";
import { renderUiText, type ThoughtView } from "../app-view.js";
import { useI18n } from "../i18n/index.js";
export function ThoughtBubble({ view }: { readonly view: ThoughtView | null }): ReactElement | null {
  const { t } = useI18n();
  return view === null ? null : <aside className={`thought-bubble${view.leaving ? " is-leaving" : ""}`} aria-live="polite"><span>{renderUiText(view.text, t)}</span></aside>;
}
