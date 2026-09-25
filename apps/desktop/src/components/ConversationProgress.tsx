import { useEffect, useState, type ReactElement } from "react";

import { renderUiText } from "../app-view.js";
import type { ConversationProgressView } from "../composer-view.js";
import { formatTime } from "../view-model.js";
import { useI18n } from "../i18n/index.js";

interface Props {
  readonly progress: ConversationProgressView;
  readonly onToggle: () => void;
  readonly onCancel: () => void;
  readonly onLayoutChange: () => void;
}

export function ConversationProgress({ progress, onToggle, onCancel, onLayoutChange }: Props): ReactElement | null {
  const { locale, t } = useI18n();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!progress.visible || progress.startedAt === null) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [progress.visible, progress.startedAt]);
  useEffect(() => {
    if (progress.expanded) onLayoutChange();
  }, [onLayoutChange, progress.expanded, progress.history.length]);
  if (!progress.visible || progress.status === null) return null;
  const receiving = progress.inputId === null;
  const elapsed = progress.startedAt === null ? null : elapsedText(progress.startedAt, now);
  return <section className="conversation-progress" aria-live="polite" aria-label={t("conversation.progress.label")}>
    <div className="conversation-progress-line">
      <button className="conversation-progress-summary" type="button" aria-expanded={progress.expanded} disabled={receiving} onClick={onToggle}>
        <span>{renderUiText(progress.status, t)}</span>
        {elapsed === null ? null : <time dateTime={progress.startedAt ?? undefined}>{t("conversation.progress.elapsed", { value: elapsed })}</time>}
        {receiving ? null : <span aria-hidden="true">{progress.expanded ? "⌃" : "⌄"}</span>}
      </button>
      {progress.canCancel ? <button className="conversation-progress-stop" type="button" onClick={onCancel}>{t("conversation.progress.stop")}</button> : null}
    </div>
    {progress.expanded ? <ol className="conversation-progress-history">
      {progress.history.map((entry, index) => <li key={`${entry.at}-${index}`}>
        <time dateTime={entry.at}>{formatTime(entry.at, locale)}</time>
        <span>{renderUiText(entry.text, t)}</span>
      </li>)}
    </ol> : null}
  </section>;
}

function elapsedText(startedAt: string, now: number): string | null {
  const started = Date.parse(startedAt);
  if (Number.isNaN(started)) return null;
  const seconds = Math.max(0, Math.floor((now - started) / 1000));
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}
