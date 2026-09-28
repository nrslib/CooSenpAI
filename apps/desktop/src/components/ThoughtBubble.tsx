import type { ReactElement } from "react";
import { renderUiText, type ThoughtView } from "../app-view.js";
import { useI18n } from "../i18n/index.js";
export function ThoughtBubble({ view, bubbleTail = false }: { readonly view: ThoughtView | null; readonly bubbleTail?: boolean }): ReactElement | null {
  const { t } = useI18n();
  return view === null ? null : <aside className={`thought-bubble${view.leaving ? " is-leaving" : ""}${bubbleTail ? " has-tail" : ""}`} aria-live="polite">
    <span className="thought-bubble__surface" aria-hidden="true" />
    {bubbleTail && <>
      <span className="thought-bubble-tail thought-bubble-tail--near" aria-hidden="true" />
      <span className="thought-bubble-tail thought-bubble-tail--far" aria-hidden="true" />
    </>}
    <span className="thought-bubble__text">{renderUiText(view.text, t)}</span>
  </aside>;
}
