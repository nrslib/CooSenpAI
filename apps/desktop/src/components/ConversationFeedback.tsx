import { useRef, type ReactElement } from "react";
import type { FeedbackAction, FeedbackRow } from "../composer-view.js";
import { useI18n } from "../i18n/index.js";

interface Props {
  readonly id: string;
  readonly row: FeedbackRow;
  readonly onInput: (action: FeedbackAction, value?: string) => void;
}

export function ConversationFeedback({ id, row, onInput }: Props): ReactElement {
  const { t } = useI18n();
  const draft = useRef<HTMLTextAreaElement>(null);
  return <div className="message-feedback" aria-label={t("feedback.label")} aria-busy={row.busy}>
    <div className="message-feedback-actions">
      <button type="button" aria-pressed={row.sign === "positive"} disabled={row.busy} onClick={() => onInput("good")}>Good</button>
      <button type="button" aria-pressed={row.sign === "negative"} disabled={row.busy} onClick={() => onInput("bad")}>Bad</button>
      {row.sign !== null && !row.editing ? <button type="button" disabled={row.busy} onClick={() => onInput("comment")}>{t(row.comment === null ? "feedback.addComment" : "feedback.editComment")}</button> : null}
    </div>
    {row.editing ? <div className="message-feedback-comment">
      <label htmlFor={`feedback-comment-${id}`}>{t("feedback.comment")}</label>
      <textarea id={`feedback-comment-${id}`} ref={draft} maxLength={500} defaultValue={row.draft} disabled={row.busy} onChange={(event) => onInput("edit", event.target.value)} />
      <div className="message-feedback-actions"><button type="button" disabled={row.busy} onClick={() => onInput("save", draft.current!.value)}>{t("feedback.save")}</button><button type="button" disabled={row.busy} onClick={() => onInput("dismiss")}>{t("common.cancel")}</button></div>
    </div> : row.comment === null ? null : <p className="message-feedback-saved-comment">{row.comment}</p>}
    {row.busy ? <small role="status">{t("feedback.saving")}</small> : row.status === null ? null : <small role="status">{row.status}</small>}
    {row.error === null ? null : <small role="alert">{row.error}</small>}
  </div>;
}
