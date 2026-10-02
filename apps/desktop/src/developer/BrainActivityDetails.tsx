import { useEffect, useState, type ReactElement } from "react";
import type { BrainActivityModule, BrainActivityObservation, BrainActivityPreview, BrainModuleEvaluation } from "../brain-activity.js";
import { useI18n } from "../i18n/index.js";
import type { JudgeDecision } from "../types.js";

function Evaluation({ value }: { readonly value: BrainModuleEvaluation | JudgeDecision | null }): ReactElement {
  const { t } = useI18n();
  if (value === null) return <p>{t("brain.decisionMissing")}</p>;
  return <p>{t(`brain.action.${value.action}`)} / {t("brain.readiness")}: {value.readiness} / {t("brain.novelty")}: {value.novelty?.toFixed(4) ?? "—"} / {t("brain.relevance")}: {value.relevance?.toFixed(4) ?? "—"}</p>;
}
function Preview({ value, inputId }: { readonly value: BrainActivityPreview | null; readonly inputId: string }): ReactElement {
  const { t } = useI18n();
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (value === null) { setUrl(null); return; }
    const bytes = Uint8Array.from({ length: value.pngHex.length / 2 }, (_, index) => Number.parseInt(value.pngHex.slice(index * 2, index * 2 + 2), 16));
    const next = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    setUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [value]);
  return <div className="brain-preview">{value === null || url === null ? <p>{t("brain.imageMissing")}</p> : <img src={url} width={value.width} height={value.height} alt={t("brain.image", { id: inputId })} />}</div>;
}
export function BrainActivityDetails({ observation, module }: { readonly observation: BrainActivityObservation; readonly module: BrainActivityModule }): ReactElement {
  const { t } = useI18n();
  const decision = observation.decision?.inputId === observation.inputId ? observation.decision : null;
  const preview = module.preview?.imageSha256 === module.activity?.image_sha256 ? module.preview : null;
  return <div className="brain-decision">
    <section className="brain-image-section"><h3>{t("brain.imageHeading")}</h3><Preview key={module.moduleIndex} value={preview} inputId={observation.inputId} /></section>
    <section className="brain-verdict-section">
    <h3>{t("brain.overallDecision")}</h3><Evaluation value={decision} />
    {observation.mode === null ? null : <p>{t(observation.mode === "shadow" ? "brain.shadowDecision" : observation.mode === "follow" ? "brain.followDecision" : "brain.fallbackDecision")}</p>}
    {decision?.holdReason === undefined ? null : <p>{t("brain.reason")}: {decision.holdReason}</p>}
    {decision?.fallbackReason === undefined ? null : <p>{t("brain.reason")}: {decision.fallbackReason}</p>}
    <h3>{t("brain.moduleDecision", { number: module.moduleIndex + 1 })}</h3><Evaluation value={module.evaluation} />
    {module.holdReason === null ? null : <p>{t("brain.reason")}: {module.holdReason}</p>}
    {module.error === null ? null : <p>{t("brain.moduleError")}: {module.error}</p>}
    </section>
  </div>;
}
