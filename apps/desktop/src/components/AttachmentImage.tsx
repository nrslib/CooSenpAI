import { useEffect, useState, type ReactElement } from "react";

import { desktopApi } from "../ipc.js";
import type { IpcResult } from "../types.js";

export function attachmentIsExpired(result: IpcResult<readonly number[]>): boolean {
  return !result.ok;
}

export function AttachmentImage({ path }: { readonly path: string }): ReactElement {
  const [source, setSource] = useState<string>();
  const [expired, setExpired] = useState(false);
  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    let active = true;
    let objectUrl: string | undefined;
    setExpired(false);
    void desktopApi.readAttachment(path).then((result) => {
      if (!active) return;
      if (!result.ok) {
        setExpired(true);
        return;
      }
      const bytes = new Uint8Array(result.value);
      objectUrl = URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: "image/png" }));
      setSource(objectUrl);
    });
    return () => {
      active = false;
      if (objectUrl !== undefined) URL.revokeObjectURL(objectUrl);
    };
  }, [path]);
  if (expired) return <div className="attachment-expired">期限切れ</div>;
  if (source === undefined) return <div className="attachment-loading">画像を読み込み中…</div>;
  return <>
    <button className="attachment-link" type="button" onClick={() => setExpanded(true)} aria-label="送信した画面を拡大">
      <img className="attachment-thumbnail" src={source} alt="送信した画面" />
    </button>
    {expanded ? <div className="attachment-overlay" role="dialog" aria-modal="true" aria-label="送信した画面" onClick={() => setExpanded(false)}>
      <button type="button" className="attachment-close" onClick={() => setExpanded(false)}>閉じる</button>
      <img src={source} alt="送信した画面の拡大表示" onClick={(event) => event.stopPropagation()} />
    </div> : null}
  </>;
}
