import { useState, type ReactElement } from "react";

import { attachmentHistory } from "../attachment-history.js";
import type { AppSnapshot } from "../types.js";
import { attachmentTextView, formatTime } from "../view-model.js";
import { AttachmentImage } from "./AttachmentImage.js";

export function AttachmentHistory({ snapshot }: { readonly snapshot: AppSnapshot }): ReactElement {
  const [expandedText, setExpandedText] = useState<string>();
  const items = attachmentHistory(snapshot.conversation);
  return <section className="attachment-history" aria-label="添付の履歴">
    <div className="history-heading"><h2>履歴</h2><p>これまでに渡した画像と文章</p></div>
    <div className="history-list">
      {items.length === 0 ? <div className="history-empty">まだ添付はありません。</div> : null}
      {items.map(({ input, reply, kind }) => {
        const text = input.attachmentText === undefined ? undefined : attachmentTextView(input.attachmentText);
        return <article className="history-item" key={input.id}>
          <div className="history-meta"><span>{kind === "image" ? "画像" : "文章"}</span><time dateTime={input.createdAt}>{formatTime(input.createdAt)}</time></div>
          {input.attachmentPath === undefined ? null : <AttachmentImage path={input.attachmentPath} />}
          {text === undefined ? null : <button className="history-text" type="button" onClick={() => setExpandedText(input.attachmentText)}>{text.preview}{text.previewTruncated ? "…" : ""}{text.truncationNotice === undefined ? null : <small>{text.truncationNotice}</small>}</button>}
          <p className="history-request">{input.message}</p>
          {reply === undefined ? <p className="history-reply pending">返事はまだありません。</p> : <div className="history-reply"><strong>{snapshot.companionDisplayName}</strong><p>{reply.message}</p></div>}
        </article>;
      })}
    </div>
    {expandedText === undefined ? null : <div className="attachment-overlay text-attachment-overlay" role="dialog" aria-modal="true" aria-label="渡した文章" onClick={() => setExpandedText(undefined)}><button type="button" className="attachment-close" onClick={() => setExpandedText(undefined)}>閉じる</button><pre onClick={(event) => event.stopPropagation()}>{expandedText}</pre></div>}
  </section>;
}
