import { useEffect, useState, type ReactElement } from "react";

import type { AppSnapshot, DebugDetail } from "../types.js";
import { effectiveAssertiveness, nowLine } from "../view-model.js";
import { NowDetails } from "./NowDetails.js";

export function NowLine({ snapshot, onShowDebug, onAssertiveness }: { readonly snapshot: AppSnapshot; readonly onShowDebug: (detail: DebugDetail) => void; readonly onAssertiveness: (value: "low" | "normal" | "high") => void }): ReactElement {
  const [open, setOpen] = useState(false);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const selectedAssertiveness = effectiveAssertiveness(snapshot, now);
  return <section className={`now-section${open ? " is-open" : ""}`}>
    <div className="assertiveness-chips" role="group" aria-label="会話の距離感">
      <span className="assertiveness-temporary-note">いまだけ</span>
      {([ ["low", "集中したい"], ["normal", "話しかけてOK"], ["high", "積極的に"] ] as const).map(([value, label]) => <button key={value} type="button" aria-pressed={selectedAssertiveness === value} onClick={() => onAssertiveness(value)}>{label}</button>)}
    </div>
    <button className="now-line" type="button" aria-expanded={open} aria-controls="now-details" onClick={() => setOpen((value) => !value)}>
      <span>{nowLine(snapshot, now)}</span>
      <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 6 4 4 4-4" /></svg>
    </button>
    {open ? <NowDetails snapshot={snapshot} onShowDebug={onShowDebug} /> : null}
  </section>;
}
