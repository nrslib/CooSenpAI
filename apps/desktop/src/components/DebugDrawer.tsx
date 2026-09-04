import type { ReactElement } from "react";

import type { DebugDetail } from "../types.js";

export function DebugDrawer({ detail, close }: { readonly detail: DebugDetail; readonly close: () => void }): ReactElement {
  return <aside className="debug-drawer">
    <div className="drawer-heading"><h3>デバッグ詳細</h3><button type="button" onClick={close}>閉じる</button></div>
    <p>送信画像: {detail.imageFiles.length === 0 ? "なし" : detail.imageFiles.join(", ")}</p>
    <p>OCR: {detail.ocrPreview ?? "なし"}</p>
    <h4>視覚の返答</h4>
    <pre>{detail.observerResponse === undefined ? "なし" : JSON.stringify(detail.observerResponse, null, 2)}</pre>
    <h4>companion へ渡した文脈</h4>
    <pre>{detail.companionContext ?? "なし"}</pre>
  </aside>;
}
