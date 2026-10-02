import type { ReactElement } from "react";
import { useI18n } from "../i18n/index.js";
import type { SpeakerDetailsView } from "./speaker-details-presenter.js";

export function SpeakerDecisionDetails({ details }: { readonly details: SpeakerDetailsView }): ReactElement {
  const { t } = useI18n();
  return <div className="log-speaker-decisions">
    <p>{details.currentStatus}</p>
    {details.missingInitial ? <p className="field-help">{t(details.decisions.length === 0
      ? "details.speakerNoDetails" : "details.speakerNoInitialDetails")}</p> : null}
    {details.decisions.map((decision) => <section className="log-speaker-decision" key={decision.key}>
      <h4>{decision.phase} · {decision.status}</h4>
      <p className="field-help">{decision.decisionKind} · {t("details.speakerVersion")}: {decision.version} · {t("details.speakerDecisionTime")}: {decision.decidedAt}</p>
      <p>{decision.reason}</p>
      {decision.roleNotice === null ? null : <p>{decision.roleNotice}</p>}
      {decision.pendingSummary === null ? null : <p>{decision.pendingSummary}</p>}
      {decision.unknownSummary === null ? null : <p>{decision.unknownSummary}</p>}
      {decision.noRepresentatives === null ? null : <p>{decision.noRepresentatives}</p>}
      <p className="field-help">{decision.interval}</p>
      <p className="field-help">{decision.criteria}</p>
      <p className="field-help">{decision.margin}</p>
      <p className="field-help">{decision.evidence}</p>
      {decision.recent === null ? null : <p className="field-help">{decision.recent}</p>}
      {decision.recentComparisons === null ? null
        : decision.recentComparisons.length === 0 ? <p className="field-help">{t("details.speakerNoRecentComparisons")}</p>
          : <table>
            <caption>{decision.recentComparisonTitle}</caption>
            <thead><tr><th>{t("details.speakerComparisonSource")}</th><th>{t("details.speakerRecentSimilarity")}</th><th>{t("details.speakerComparisonBasis")}</th></tr></thead>
            <tbody>{decision.recentComparisons.map((comparison) => <tr key={comparison.key}>
              <td>
                {comparison.timeValue === null ? null : <time dateTime={comparison.timeValue}>{comparison.timeLabel}</time>}
                {comparison.sourceMissing ? <p className="field-help">{t("details.speakerComparisonSourceMissing")}</p> : null}
                {comparison.excerpt === null ? null : <p>{comparison.excerpt}</p>}
                <small>{comparison.sourceLabel}</small>
              </td>
              <td>{comparison.score}</td>
              <td>{comparison.basis}</td>
            </tr>)}</tbody>
          </table>}
      {decision.candidates.length === 0 ? null : <table>
        <caption>{decision.candidateTitle}</caption>
        <thead><tr><th>{t("details.speakerCandidate")}</th><th>{decision.candidateSimilarity}</th><th>{t("details.speakerRankReason")}</th></tr></thead>
        <tbody>{decision.candidates.map((candidate) => <tr key={candidate.id}>
          <td title={candidate.id}>{candidate.label}</td><td>{candidate.score}</td><td>{candidate.rankReason}</td>
        </tr>)}</tbody>
      </table>}
      {decision.support.length === 0 ? null : <h5>{decision.supportTitle}</h5>}
      {decision.supportCriteria === null ? null : <p>{decision.supportCriteria}</p>}
      {decision.support.map((sample) => <p className="field-help" key={sample.key}>{sample.label}: {sample.score}</p>)}
    </section>)}
  </div>;
}
