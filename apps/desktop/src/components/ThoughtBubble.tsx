import { useEffect, useState, type ReactElement } from "react";

import type { AppSnapshot } from "../types.js";
import { thoughtBubbleText } from "../view-model.js";

export function ThoughtBubble({ snapshot }: { readonly snapshot: AppSnapshot }): ReactElement | null {
  const next = snapshot.config.ui.thoughtBubble ? thoughtBubbleText(snapshot) : undefined;
  const [displayed, setDisplayed] = useState(next);
  const [leaving, setLeaving] = useState(false);
  useEffect(() => {
    if (next !== undefined) {
      setDisplayed(next);
      setLeaving(false);
      return;
    }
    if (displayed === undefined) return;
    setLeaving(true);
    const timer = window.setTimeout(() => {
      setDisplayed(undefined);
      setLeaving(false);
    }, 200);
    return () => window.clearTimeout(timer);
  }, [next, displayed]);
  return displayed === undefined ? null : <aside className={`thought-bubble${leaving ? " is-leaving" : ""}`} aria-live="polite"><span>{displayed}</span></aside>;
}
