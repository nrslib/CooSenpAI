import { useEffect, useRef, type ReactElement } from "react";

import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";
import { desktopApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";

export function AttachmentImage({ path }: { readonly path: string }): ReactElement {
  const { t } = useI18n();
  const resources = useRef({ disposed: false, urls: new Set<string>() });
  const presenter = usePanelPresenter<{ source: string | null; expired: boolean; expanded: boolean }, PanelCommand & { payload: string }>("attachment", path, async (command) => {
    const allocated = resources.current;
    const result = await desktopApi.readAttachment(command.payload);
    if (allocated.disposed) throw new DOMException("Attachment view disposed", "AbortError");
    if (!result.ok) return result;
    const bytes = new Uint8Array(result.value);
    const source = URL.createObjectURL(new Blob([bytes.buffer], { type: "image/png" }));
    allocated.urls.add(source);
    return { ok: true, value: source };
  });
  useEffect(() => presenter.send({ type: "change", value: path }), [path, presenter.send]);
  useEffect(() => {
    const allocated = { disposed: false, urls: new Set<string>() };
    resources.current = allocated;
    return () => { allocated.disposed = true; for (const url of allocated.urls) URL.revokeObjectURL(url); allocated.urls.clear(); };
  }, []);
  const source = presenter.state?.source;
  const expired = presenter.state?.expired;
  const expanded = presenter.state?.expanded;
  const action = (name: string): void => presenter.send({ type: "action", name, value: null });
  if (presenter.error !== undefined) return <p role="alert">{presenter.error}</p>;
  if (expired) return <div className="attachment-expired">{t("attachment.expired")}</div>;
  if (source == null) return <div className="attachment-loading">{t("attachment.loading")}</div>;
  return <>
    <button className="attachment-link" type="button" onClick={() => action("expand")} aria-label={t("attachment.enlarge")}>
      <img className="attachment-thumbnail" src={source} alt={t("attachment.alt")} />
    </button>
    {expanded ? <div className="attachment-overlay" role="dialog" aria-modal="true" aria-label={t("attachment.alt")} onClick={() => action("close")}>
      <button type="button" className="attachment-close" onClick={() => action("close")}>{t("common.close")}</button>
      <img src={source} alt={t("attachment.expandedAlt")} onClick={(event) => event.stopPropagation()} />
    </div> : null}
  </>;
}
