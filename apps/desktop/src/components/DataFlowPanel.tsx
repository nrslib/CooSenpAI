import { useEffect, useRef, useState, type ReactElement } from "react";

import {
  DATAFLOW_KIND_LABELS,
  type DataFlowEvent,
  type DataFlowKindFilter,
} from "../dataflow.js";
import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";
import { useI18n, type TranslationKey } from "../i18n/index.js";
import { formatTime } from "../view-model.js";

const KIND_FILTERS = [
  { id: "all", label: "details.dataflowFilterAll" },
  { id: "visual", label: "details.dataflowFilterVisual" },
  { id: "hearing", label: "details.dataflowFilterHearing" },
  { id: "companion", label: "details.dataflowFilterSpeech" },
] as const satisfies readonly { readonly id: DataFlowKindFilter; readonly label: TranslationKey }[];

function DataFlowRow({ event, expanded, onToggle }: { readonly event: DataFlowEvent; readonly expanded: boolean; readonly onToggle: () => void }): ReactElement {
  const { locale, t } = useI18n();
  const content = <>
    <time dateTime={event.at}>{formatTime(event.at, locale)}</time>
    <span className={`dataflow-kind dataflow-kind-${event.kind}`}>{t(DATAFLOW_KIND_LABELS[event.kind])}</span>
    <span className="dataflow-summary">{event.summary}</span>
  </>;
  if (event.detail === undefined) {
    return <div className={`dataflow-row dataflow-row-${event.kind}`}>{content}</div>;
  }
  return <details open={expanded} className={`dataflow-row dataflow-row-${event.kind}`}>
    <summary onClick={(event) => { event.preventDefault(); onToggle(); }}>{content}</summary>
    <pre className="dataflow-detail">{event.detail}</pre>
  </details>;
}

export function DataFlowPanel({ events, loadError }: {
  readonly events: readonly DataFlowEvent[];
  readonly loadError?: string;
}): ReactElement {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const presenter = usePanelPresenter<{ filter: DataFlowKindFilter; visible: readonly DataFlowEvent[]; expanded: readonly string[] }, PanelCommand>("dataflow", events, async (command) => {
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
  const kindFilter = presenter.state?.filter ?? "all";
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
          aria-pressed={kindFilter === filter.id}
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
      {visible.map((event) => <DataFlowRow key={event.id} event={event} expanded={presenter.state?.expanded.includes(event.id) === true} onToggle={() => action("toggle", event.id)} />)}
    </div>
  </div>;
}
