import { useI18n } from "../i18n/index.js";
import { useEffect, useState, type ReactElement } from "react";
import { desktopApi } from "../ipc.js";
import type { WorkRootStatus } from "../work.js";
import type { WorkApprovalView } from "../work-approval-view.js";
import "./WorkApproval.css";

interface Props {
  readonly inputId: string;
  readonly onLayoutChange?: () => void;
}

export function WorkApproval({ inputId, onLayoutChange }: Props): ReactElement | null {
  const { t } = useI18n();
  const [view, setView] = useState<WorkApprovalView>();
  useEffect(() => {
    let disposed = false;
    const subscription = desktopApi.subscribeWorkApprovalView((next) => { if (next.inputId === inputId) setView(next); });
    void subscription.ready.then(() => { if (!disposed) void desktopApi.workApprovalInput({ type: "mounted", inputId }); });
    return () => { disposed = true; subscription.dispose(); void desktopApi.workApprovalInput({ type: "unmounted", inputId }); };
  }, [inputId]);
  useEffect(() => { onLayoutChange?.(); }, [view?.layoutRequest]);
  const send = (action: import("../work-approval-view.js").WorkAction) => { void desktopApi.workApprovalInput({ type: "action", inputId, approvalId: view?.approval?.id ?? null, action }); };
  if (view === undefined || view.inputId !== inputId) return null;
  const { approval, awaiting, busy, error } = view;
  if (!view.visible) return error ? <p role="alert">{error}</p> : null;
  const kindLabel = approval?.kind === "work" ? t("work.kindWork") : t("work.kindInvestigate");
  const rootLabel = (root: WorkRootStatus): string => {
    switch (root.status) {
      case "inside": return t("work.rootInside", { root: root.root });
      case "outside": return t("work.rootOutside");
    }
  };
  return <section className="work-approval-card" aria-label={t("work.card")}>
    <p role="status">{awaiting ? t("work.pending") : t("work.running")}</p>
    <button type="button" onClick={() => send("cancel")}>{t("work.stop")}</button>
    {approval && awaiting ? <div className="work-approval" role="region" aria-label={t("work.request")}>
      <strong>{t("work.permit", { operation: kindLabel })}</strong>
      <p>{approval.target}</p>
      <p>{rootLabel(approval.root)}</p>
      <p>{approval.reason}</p>
      {approval.decisionReason ? <p>{approval.decisionReason}</p> : null}
      {approval.status === "reviewing" ? <p role="status">{t("work.reviewing")}</p> : null}
      <div className="button-row">
        <button type="button" disabled={busy} onClick={() => send("allow")}>{t("work.allow")}</button>
        {approval.root.status === "outside" ? <button type="button" disabled={busy} onClick={() => send("addRootAndAllow")}>{t("work.allowAndAddRoot")}</button> : null}
        <button type="button" disabled={busy} onClick={() => send("deny")}>{t("work.deny")}</button>
        <button type="button" disabled={busy} onClick={() => send("toggleMode")}>{view.manual ? t("work.switchAuto") : t("work.switchManual")}</button>
      </div>
      <p className="field-help">{t("work.saved")}</p>
    </div> : null}
    {error ? <p role="alert">{error}</p> : null}
  </section>;
}
