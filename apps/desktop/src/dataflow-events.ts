import { renderDataFlowRecord, type DataFlowEvent, type DataFlowRecord } from "./dataflow.js";
import type { Locale } from "./i18n/index.js";

export interface DataFlowEventsState {
  readonly events: readonly DataFlowEvent[];
  readonly loadError?: string;
}

export function renderDataFlowEvents(state: { readonly events: readonly DataFlowRecord[]; readonly loadError: string | null }, locale: Locale): DataFlowEventsState {
  return { events: state.events.map((record) => renderDataFlowRecord(record, locale)), loadError: state.loadError ?? undefined };
}
