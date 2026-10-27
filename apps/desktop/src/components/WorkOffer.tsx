import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { desktopApi } from "../ipc.js";
import type { WorkOfferAction, WorkOffer as WorkOfferValue } from "../types.js";
import "./WorkOffer.css";

interface Props {
  readonly offer: WorkOfferValue;
  readonly operationGeneration: number;
  readonly busy: boolean;
  readonly error?: string | null;
}

export function WorkOffer({ offer, operationGeneration, busy, error }: Props): ReactElement {
  const action = (action: WorkOfferAction): void => {
    void desktopApi.workOfferInput({ offerId: offer.id, operationGeneration, action });
  };
  const { t } = useI18n();
  const kind = offer.proposal.kind === "work" ? t("work.kindWork") : t("work.kindInvestigate");
  return <section className="work-offer-card" aria-label={t("work.offer.label")}>
    <strong>{kind}</strong>
    <dl>
      <div><dt>{t("work.offer.cwd")}</dt><dd><code>{offer.proposal.cwd}</code></dd></div>
      <div><dt>{t("work.offer.summary")}</dt><dd>{offer.proposal.summary}</dd></div>
    </dl>
    <p className="work-offer-explanation">{t("work.offer.explanation")}</p>
    {error == null ? null : <p className="work-offer-error" role="alert">{error}</p>}
    <div className="button-row">
      <button type="button" disabled={busy} onClick={() => action("accept")}>{t("work.offer.yes")}</button>
      <button type="button" disabled={busy} onClick={() => action("decline")}>{t("work.offer.no")}</button>
    </div>
  </section>;
}

export function pendingWorkOfferForEntry(offer: WorkOfferValue | null | undefined, entryId: string): WorkOfferValue | undefined {
  return offer?.status === "proposed" && offer.entryId === entryId ? offer : undefined;
}
