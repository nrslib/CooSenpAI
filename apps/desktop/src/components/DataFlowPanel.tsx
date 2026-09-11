import { useEffect, useRef, useState, type ReactElement } from "react";

import {
  DATAFLOW_KIND_LABELS,
  DATAFLOW_MARKER_LABELS,
  type DataFlowReference,
  type DataFlowEvent,
  type DataFlowKindFilter,
} from "../dataflow.js";
import { detailsApi } from "../ipc.js";
import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";
import { useI18n, type TranslationKey } from "../i18n/index.js";
import { formatTime } from "../view-model.js";

const KIND_FILTERS = [
  { id: "all", label: "details.dataflowFilterAll" },
  { id: "vision", label: "details.dataflowFilterVisual" },
  { id: "hearing", label: "details.dataflowFilterHearing" },
  { id: "coo", label: "details.dataflowFilterCoo" },
] as const satisfies readonly { readonly id: DataFlowKindFilter; readonly label: TranslationKey }[];

function DataFlowRow({ event, expanded, onToggle, onOpenPath }: { readonly event: DataFlowEvent; readonly expanded: boolean; readonly onToggle: () => void; readonly onOpenPath: (path: string) => void }): ReactElement {
  const { locale, t } = useI18n();
  const referenceLabel = (reference: DataFlowReference): string => {
    const value = reference.path ?? reference.id;
    return `${t(reference.type === "frame"
      ? "details.dataflowReferenceFrame"
      : reference.type === "transcript"
        ? "details.dataflowReferenceTranscript"
        : reference.type === "conversation"
          ? "details.dataflowReferenceConversation"
          : "details.dataflowReferenceObservation")}: ${value}`;
  };
  const content = <>
    <time dateTime={event.at}>{formatTime(event.at, locale)}</time>
    <span className={`dataflow-kind dataflow-kind-${event.subject}`}>{t(DATAFLOW_KIND_LABELS[event.subject])}</span>
    <span className="dataflow-marker">{t(DATAFLOW_MARKER_LABELS[event.marker])}</span>
    <span className="dataflow-summary">{event.summary}</span>
  </>;
  const references = event.references.map((reference) => {
    const path = reference.path;
    return <span className="dataflow-reference" key={`${reference.type}:${reference.id}:${path ?? ""}`}>
      <span>{referenceLabel(reference)}{reference.observationKind === undefined ? "" : ` · ${t(DATAFLOW_MARKER_LABELS[reference.observationKind])}`}{reference.source == null ? "" : ` · ${reference.source === "microphone" ? "mic" : reference.source}`}</span>
      {reference.text == null ? null : <pre>{reference.text}</pre>}
      {path === undefined || reference.available !== true ? null : <button type="button" onClick={() => onOpenPath(path)}>{t("details.dataflowOpenReference")}</button>}
    </span>;
  });
  const detail = <>
    {event.references.length === 0 ? null : <div className="dataflow-references">{references}</div>}
    {event.detail === undefined ? null : <pre className="dataflow-detail">{event.detail}</pre>}
  </>;
  if (event.detail === undefined && event.references.length === 0) {
    return <div className={`dataflow-row dataflow-row-${event.subject}`}>{content}</div>;
  }
  return <details open={expanded} className={`dataflow-row dataflow-row-${event.subject}`}>
    <summary onClick={(event) => { event.preventDefault(); onToggle(); }}>{content}</summary>
    {detail}
  </details>;
}

export function DataFlowPanel({ events, loadError }: {
  readonly events: readonly DataFlowEvent[];
  readonly loadError?: string;
}): ReactElement {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const presenter = usePanelPresenter<{ filters: readonly DataFlowKindFilter[]; visible: readonly DataFlowEvent[]; expanded: readonly string[] }, PanelCommand>("dataflow", events, async (command) => {
    if (command.kind === "openPath") {
      const payload = command.payload;
      if (typeof payload !== "string") throw new Error("Invalid dataflow path command");
      return detailsApi.openDataFlowPath(payload);
    }
    if (command.kind !== "scrollEnd") throw new Error(`Unknown dataflow command: ${command.kind}`);
    await new Promise<void>((resolve) => requestAnimationFrame(() => {
      const list = listRef.current;
      if (list !== null) list.scrollTop = list.scrollHeight;
      resolve();
    }));
    return { ok: true, value: null };
  });
  useEffect(() => presenter.send({ type: "change", value: events }), [events, presenter.send]);
  const action = (name: string, value: unknown): void => presenter.send({ type: "action", name, value });
  const filters = presenter.state?.filters ?? [];
  const visible = presenter.state?.visible ?? [];
  const handleScroll = (): void => {
    const list = listRef.current;
    if (list !== null) action("scroll", list.scrollHeight - list.scrollTop - list.clientHeight);
  };

  return <div className="dataflow-panel">
    <p className="dataflow-notice" role="note">{t("details.dataflowNotice")}</p>
    <div className="dataflow-controls">
      <div className="dataflow-filters" role="group" aria-label={t("details.dataflowFilterLabel")}>
        {KIND_FILTERS.map((filter) => <button
          key={filter.id}
          type="button"
          className="dataflow-filter"
          aria-pressed={filter.id === "all" ? filters.length === 0 : filters.includes(filter.id)}
          onClick={() => action("filter", filter.id)}
        >{t(filter.label)}</button>)}
      </div>
      <input
        className="dataflow-search"
        type="search"
        value={query}
        placeholder={t("details.dataflowSearchPlaceholder")}
        aria-label={t("details.dataflowSearchPlaceholder")}
        onChange={(event) => { setQuery(event.target.value); action("query", event.target.value); }}
      />
    </div>
    <div className="dataflow-list" ref={listRef} onScroll={handleScroll}>
      {loadError === undefined ? null : <p className="dataflow-empty" role="alert">{loadError}</p>}
      {loadError === undefined && visible.length === 0 ? <p className="dataflow-empty">{t("details.dataflowEmpty")}</p> : null}
      {visible.map((event) => <DataFlowRow key={event.id} event={event} expanded={presenter.state?.expanded.includes(event.id) === true} onToggle={() => action("toggle", event.id)} onOpenPath={(path) => action("open", path)} />)}
    </div>
  </div>;
}
