import { useEffect, useLayoutEffect, useRef, type ReactElement, type RefObject } from "react";

import { applyAppearance } from "../appearance.js";
import { CompanionEmotionsPanel } from "../components/CompanionEmotionsPanel.js";
import { ConfirmationDialog } from "../components/ConfirmationDialog.js";
import { DataFlowPanel } from "../components/DataFlowPanel.js";
import { renderDataFlowEvents } from "../dataflow-events.js";
import type { DataFlowRecord } from "../dataflow.js";
import { usePanelPresenter, deliverPanelUpdates, type PanelCommand } from "../usePanelPresenter.js";
import { NowDetails } from "../components/NowDetails.js";
import { DebugDrawer } from "../components/DebugDrawer.js";
import { detailsApi, ipcTransportError, setIpcLocale } from "../ipc.js";
import { I18nProvider, t, useI18n, type Locale, type TranslationKey } from "../i18n/index.js";
import type { AppSnapshot, ConversationLogState, DebugDetail, IpcResult } from "../types.js";
import { ConversationLogPanel } from "./ConversationLogPanel.js";
import { presentSpeakerDetails } from "./speaker-details-presenter.js";
import { ConversationsPanel } from "./ConversationsPanel.js";

export type DetailsTab = "state" | "emotions" | "conversation" | "dataflow" | "log";

interface DetailTabDefinition {
  readonly id: DetailsTab;
  readonly label: TranslationKey;
}

export const DETAILS_TABS = [
  { id: "state", label: "details.state" },
  { id: "emotions", label: "details.emotions" },
  { id: "conversation", label: "details.conversation" },
  { id: "dataflow", label: "details.dataflow" },
  { id: "log", label: "details.conversationLog" },
] as const satisfies readonly DetailTabDefinition[];

interface DetailsView {
  readonly snapshot: AppSnapshot | null;
  readonly activeTab: DetailsTab;
  readonly debugDetail: DebugDetail | null;
  readonly switching: boolean;
  readonly resetting: boolean;
  readonly resetError: string | null;
  readonly error: string | null;
  readonly dataFlow: { readonly events: readonly DataFlowRecord[]; readonly loadError: string | null };
  readonly conversationLog: ConversationLogState;
}
interface DetailsContentProps {
  readonly state: DetailsView | undefined;
  readonly loadError?: string;
  readonly action: (name: string, value?: unknown) => void;
  readonly logListRef: RefObject<HTMLDivElement | null>;
  readonly logScrollGeneration: { current: number };
}

function DetailsContent({ state, loadError, action, logListRef, logScrollGeneration }: DetailsContentProps): ReactElement {
  const { locale, t } = useI18n();
  const snapshot = state?.snapshot ?? undefined;
  const activeTab = state?.activeTab ?? "state";
  const dataFlow = state === undefined ? undefined : renderDataFlowEvents(state.dataFlow, locale);
  const debugDetail = state?.debugDetail ?? undefined;
  const panelId = `details-panel-${activeTab}`;

  const panel = snapshot === undefined ? null : activeTab === "state"
    ? <section className="details-tab-panel" role="tabpanel" id={panelId} aria-labelledby="details-tab-state">
      <h2 id="details-state-heading">{t("details.state")}</h2>
      <NowDetails snapshot={snapshot} onShowDebug={(detail) => action("debug", detail)} />
    </section>
    : activeTab === "emotions"
      ? <section className="details-tab-panel details-emotions-panel" role="tabpanel" id={panelId} aria-labelledby="details-tab-emotions">
        <h2 id="details-emotions-heading">{t("details.emotions")}</h2>
        <CompanionEmotionsPanel displayName={snapshot.companionDisplayName} emotions={snapshot.companionEmotions} enabled={snapshot.config.companion.emotionsEnabled} pending={state?.resetting === true} error={state?.resetError ?? undefined} onReset={() => action("resetEmotions")} open />
      </section>
      : activeTab === "dataflow"
        ? <section className="details-tab-panel details-dataflow-panel" role="tabpanel" id={panelId} aria-labelledby="details-tab-dataflow">
          <h2 id="details-dataflow-heading">{t("details.dataflow")}</h2>
          <DataFlowPanel events={dataFlow?.events ?? []} loadError={dataFlow?.loadError} />
        </section>
        : activeTab === "log"
          ? <section className="details-tab-panel details-dataflow-panel" role="tabpanel" id={panelId} aria-labelledby="details-tab-log">
            <h2 id="details-log-heading">{t("details.conversationLog")}</h2>
            {state === undefined ? null : <>
              <ConversationLogPanel log={state.conversationLog} details={presentSpeakerDetails(state.conversationLog.speakerDetails, locale)} action={action} listRef={logListRef} scrollGeneration={logScrollGeneration} />
              {state.conversationLog.deleteConfirmation === null ? null : <ConfirmationDialog
                id="conversation-log-delete"
                title={t("details.logDeleteTitle", { date: state.conversationLog.deleteConfirmation })}
                description={t("details.logDeleteDescription", { date: state.conversationLog.deleteConfirmation })}
                cancelLabel={t("details.logDeleteCancel")}
                confirmLabel={t("details.logDeleteConfirm")}
                onCancel={() => action("logDeleteCancel")}
                onConfirm={() => action("logDeleteConfirm")}
              />}
            </>}
          </section>
          : <section className="details-tab-panel" role="tabpanel" id={panelId} aria-labelledby="details-tab-conversation">
          <h2 id="details-conversation-heading">{t("details.conversation")}</h2>
          <ConversationsPanel
            generations={snapshot.conversationGenerations}
            selectedGeneration={snapshot.selectedConversationGeneration}
            switching={state?.switching === true}
            error={state?.error ?? undefined}
            onSelect={(generation) => action("select", generation)}
          />
        </section>;

  return <main className="details-shell">
    <header className="details-header"><h1>{t("details.title")}</h1></header>
    <nav className="details-tabs" role="tablist" aria-label={t("details.tabList")}>
      {DETAILS_TABS.map((tab) => <button
        key={tab.id}
        id={`details-tab-${tab.id}`}
        className="details-tab"
        type="button"
        role="tab"
        aria-selected={activeTab === tab.id}
        aria-controls={`details-panel-${tab.id}`}
        onClick={() => action("tab", tab.id)}
      >{t(tab.label)}</button>)}
    </nav>
    {snapshot === undefined
      ? <p className="details-loading" role="status">{loadError ?? t("common.loading")}</p>
      : <div className="details-content">{panel}</div>}
    {debugDetail === undefined ? null : <DebugDrawer detail={debugDetail} close={() => action("debug", null)} />}
  </main>;
}

export function Details(): ReactElement {
  const listenersReady = useRef<Promise<IpcResult<null>>>(Promise.resolve({ ok: true, value: null }));
  const logListRef = useRef<HTMLDivElement>(null);
  const logScrollGeneration = useRef(0);
  const presenter = usePanelPresenter<DetailsView, PanelCommand & { payload: number }>("details", null, async (command) => {
    switch (command.kind) {
      case "ready": { const ready = await listenersReady.current; return ready.ok ? detailsApi.ready() : ready; }
      case "select": return detailsApi.selectConversationGeneration(command.payload);
      case "resetEmotions": return detailsApi.resetCompanionEmotions();
      case "loadLog": return detailsApi.getConversationLog((command.payload as unknown as { date: string | null }).date);
      case "deleteLogDay": return detailsApi.deleteConversationLog((command.payload as unknown as { date: string }).date);
      case "speakerRename": return detailsApi.speakerRename(command.payload as unknown as import("../types.js").SpeakerRenamePayload);
      default: throw new Error(`Unknown details command: ${command.kind}`);
    }
  });
  const snapshot = presenter.state?.snapshot;
  const locale: Locale = snapshot?.config.ui.language ?? "ja";
  const scrollToEnd = presenter.state?.conversationLog.scrollToEnd;
  useLayoutEffect(() => {
    if (scrollToEnd === undefined || scrollToEnd === null) return;
    if (logScrollGeneration.current !== scrollToEnd.generation) return;
    const list = logListRef.current;
    if (list !== null) list.scrollTop = list.scrollHeight;
  }, [scrollToEnd?.generation, scrollToEnd?.request]);
  useEffect(() => {
    const updates = detailsApi.subscribePanelUpdates(deliverPanelUpdates);
    const history = detailsApi.subscribeDataFlow((value) => presenter.send({ type: "action", name: "history", value }));
    listenersReady.current = Promise.all([updates.ready, history.ready]).then(() => ({ ok: true as const, value: null }), (cause: unknown) => ({ ok: false as const, error: { message: ipcTransportError(cause) } }));
    return () => { updates.dispose(); history.dispose(); };
  }, [presenter.send]);
  useEffect(() => {
    setIpcLocale(locale);
    document.title = t(locale, "details.title");
  }, [locale]);
  useEffect(() => {
    if (snapshot != null) applyAppearance({ theme: snapshot.config.ui.theme, font: snapshot.config.ui.font });
  }, [snapshot?.config.ui.font, snapshot?.config.ui.theme]);
  return <I18nProvider locale={locale}><DetailsContent state={presenter.state} loadError={presenter.error ?? presenter.state?.error ?? undefined}
    action={(name, value = null) => presenter.send({ type: "action", name, value })} logListRef={logListRef} logScrollGeneration={logScrollGeneration} /></I18nProvider>;
}
