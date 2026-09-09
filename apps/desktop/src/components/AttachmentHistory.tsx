import { useEffect, useMemo, type ReactElement } from "react";

import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";
import { attachmentHistory, type AttachmentHistoryItem, type AttachmentHistoryFilter } from "../attachment-history.js";
import type { AppSnapshot } from "../types.js";
import { attachmentTextView, formatTime } from "../view-model.js";
import { AttachmentImage } from "./AttachmentImage.js";
import { useI18n } from "../i18n/index.js";

export function AttachmentHistory({ snapshot }: { readonly snapshot: AppSnapshot }): ReactElement {
  const items = useMemo(() => attachmentHistory(snapshot.conversation), [snapshot.conversation]);
  const presenter = usePanelPresenter<{ filter: AttachmentHistoryFilter; expandedText: string | null; visible: readonly AttachmentHistoryItem[]; empty: boolean }, PanelCommand>("history", items, async (command) => { throw new Error(`Unknown history command: ${command.kind}`); });
  const expandedText = presenter.state?.expandedText ?? undefined;
  const filter = presenter.state?.filter ?? "all";
  const action = (name: string, value: unknown): void => presenter.send({ type: "action", name, value });
  const { locale, t } = useI18n();
  useEffect(() => presenter.send({ type: "change", value: items }), [items, presenter.send]);
  const visibleItems = presenter.state?.visible ?? [];
  return <section className="attachment-history" aria-label={t("attachmentHistory.label")}>
    <div className="history-heading"><h2>{t("attachmentHistory.title")}</h2><p>{t("attachmentHistory.description")}</p></div>
    <div className="history-filters" role="group" aria-label={t("attachmentHistory.filterLabel")}>
      <button className={filter === "all" ? "is-active" : ""} type="button" aria-pressed={filter === "all"} onClick={() => action("filter", "all")}>{t("attachmentHistory.all")}</button>
      <button className={filter === "image" ? "is-active" : ""} type="button" aria-pressed={filter === "image"} onClick={() => action("filter", "image")}>{t("attachmentHistory.image")}</button>
      <button className={filter === "text" ? "is-active" : ""} type="button" aria-pressed={filter === "text"} onClick={() => action("filter", "text")}>{t("attachmentHistory.text")}</button>
    </div>
    <div className="history-list">
      {visibleItems.length === 0 ? <div className="history-empty">{presenter.state?.empty === true ? t("attachmentHistory.empty") : t("attachmentHistory.emptyFiltered")}</div> : null}
      {visibleItems.map(({ input, reply, kind }) => {
        const text = input.attachmentText === undefined ? undefined : attachmentTextView(input.attachmentText, locale);
        return <article className="history-item" key={input.id}>
          <div className="history-meta"><span>{kind === "image" ? t("attachmentHistory.image") : t("attachmentHistory.text")}</span><time dateTime={input.createdAt}>{formatTime(input.createdAt, locale)}</time></div>
          {input.attachmentPath === undefined ? null : <AttachmentImage path={input.attachmentPath} />}
          {text === undefined ? null : <button className="history-text" type="button" onClick={() => action("expand", input.attachmentText)}>{text.preview}{text.previewTruncated ? "…" : ""}{text.truncationNotice === undefined ? null : <small>{text.truncationNotice}</small>}</button>}
          <p className="history-request">{input.message}</p>
          {reply === undefined ? <p className="history-reply pending">{t("attachmentHistory.replyPending")}</p> : <div className="history-reply"><strong>{snapshot.companionDisplayName}</strong><p>{reply.message}</p></div>}
        </article>;
      })}
    </div>
    {expandedText === undefined ? null : <div className="attachment-overlay text-attachment-overlay" role="dialog" aria-modal="true" aria-label={t("attachmentHistory.sharedText")} onClick={() => action("close", null)}><button type="button" className="attachment-close" onClick={() => action("close", null)}>{t("common.close")}</button><pre onClick={(event) => event.stopPropagation()}>{expandedText}</pre></div>}
  </section>;
}
