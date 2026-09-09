import { useEffect, useState, type ReactElement } from "react";

import type { AppSnapshot } from "../types.js";
import { effectiveAssertiveness, nowLine } from "../view-model.js";
import { InfoIcon } from "./LineIcons.js";
import { useI18n } from "../i18n/index.js";

export function NowLine({ snapshot, onOpenDetails, onAssertiveness }: { readonly snapshot: AppSnapshot; readonly onOpenDetails: () => void; readonly onAssertiveness: (value: "low" | "normal" | "high") => void }): ReactElement {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const selectedAssertiveness = effectiveAssertiveness(snapshot, now);
  const { locale, t } = useI18n();
  const detailsLabel = t("details.title");
  return <section className="now-section">
    <div className="assertiveness-chips" role="group" aria-label={t("now.distance")}>
      <span className="assertiveness-temporary-note">{t("now.temporary")}</span>
      {([ ["low", "now.low"], ["normal", "now.normal"], ["high", "now.high"] ] as const).map(([value, key]) => <button key={value} type="button" aria-pressed={selectedAssertiveness === value} onClick={() => onAssertiveness(value)}>{t(key)}</button>)}
    </div>
    <div className="now-line">
      <span>{nowLine(snapshot, now, locale)}</span>
      <button className="icon-button now-details-button" type="button" aria-label={detailsLabel} title={detailsLabel} onClick={onOpenDetails}><InfoIcon /></button>
    </div>
  </section>;
}
