import { type RefObject, type ReactElement, useEffect, useRef } from "react";

import { useI18n, type TranslationKey } from "../i18n/index.js";
import type { ConversationLogEntryView, ConversationLogState } from "../types.js";
import { formatTime, speakerDisplayName } from "../view-model.js";
import { SpeakerDecisionDetails } from "./SpeakerDecisionDetails.js";
import type { SpeakerDetailsView } from "./speaker-details-presenter.js";

const FILTERS = [
  { id: "all", label: "details.logFilterAll" },
  { id: "mic", label: "details.logFilterMic" },
  { id: "unknown", label: "details.logFilterUnknown" },
  { id: "mixed", label: "details.logFilterMixed" },
] as const satisfies readonly { readonly id: string; readonly label: TranslationKey }[];

interface ConversationLogPanelProps {
  readonly log: ConversationLogState;
  readonly details: SpeakerDetailsView | null;
  readonly action: (name: string, value?: unknown) => void;
  readonly listRef: RefObject<HTMLDivElement | null>;
  readonly scrollGeneration: { current: number };
}

// 名前は表示だけに使い、操作対象の UUID は行と選択肢から全文で渡す。
function speakerLabel(entry: ConversationLogEntryView, t: (key: TranslationKey) => string): string {
  if (entry.source === "mic") return t("details.logSpeakerSelf");
  if (entry.speakerTag !== undefined) return speakerDisplayName(entry.speakerTag, entry.speakerName);
  switch (entry.speakerStatus) {
    case "mixed": return t("details.logSpeakerMixed");
    case "unknown": return t("details.logSpeakerUnknown");
    default: return t("details.logSpeakerNotIdentified");
  }
}

// 同一観察・同一開始時刻でも、終端が異なる期間を別行として保持する。
function rowKey(entry: ConversationLogEntryView): string {
  return `${entry.observationId}:${entry.audioStartMs ?? entry.time}:${entry.audioEndMs ?? "legacy"}`;
}

function LogRow({ entry, action }: { readonly entry: ConversationLogEntryView; readonly action: (name: string, value?: unknown) => void }): ReactElement {
  const { locale, t } = useI18n();
  const speakerEditId = entry.speakerEditId;
  const detailsKey = entry.speakerDetailsKey;
  const selectSpeaker = () => {
    if (detailsKey != null) action("logEntrySelect", detailsKey);
  };
  return <div
    className={`log-row log-row-${entry.source === "mic" ? "mic" : "speaker"}${detailsKey == null ? "" : " log-row-selectable"}`}
    data-speaker-id={speakerEditId ?? undefined}
    role={detailsKey == null ? undefined : "button"}
    tabIndex={detailsKey == null ? undefined : 0}
    onClick={detailsKey == null ? undefined : selectSpeaker}
    onKeyDown={detailsKey == null ? undefined : (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectSpeaker();
      }
    }}
  >
    <time dateTime={entry.time}>{formatTime(entry.time, locale)}</time>
    <span className="log-source">{t(entry.source === "mic" ? "now.microphone" : "now.speaker")}</span>
    <span className="log-speaker" title={speakerEditId ?? undefined}>{speakerLabel(entry, t)}</span>
    <span className="log-text">{entry.text}</span>
  </div>;
}

export function ConversationLogPanel({ log, details, action, listRef, scrollGeneration }: ConversationLogPanelProps): ReactElement {
  const { t } = useI18n();
  const filter = log.filter ?? "all";
  const speakerAsideRef = useRef<HTMLElement | null>(null);
  const hasSpeakerAside = details !== null || log.speakerEditor !== null;
  const isSavingSpeakerName = log.speakerEditor?.saving === true;

  useEffect(() => {
    if (!hasSpeakerAside || isSavingSpeakerName) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (event.target instanceof Node && !speakerAsideRef.current?.contains(event.target)) {
        action("logSpeakerClose");
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.isComposing) {
        event.preventDefault();
        action("logSpeakerClose");
      }
    };
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsidePointer);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [action, hasSpeakerAside, isSavingSpeakerName]);

  return <div className="log-panel">
    <div className="dataflow-controls">
      <label className="log-filter-control">
        <span>{t("details.logDateLabel")}</span>
        <select name="logDate" className="log-filter-select" value={log.selectedDate ?? ""} disabled={log.dates.length === 0 || log.deleting}
          onChange={(event) => action("logDate", event.target.value)}>
          {log.selectedDate === null ? <option value="" /> : null}
          {log.dates.map((date) => <option key={date} value={date}>{date}</option>)}
        </select>
      </label>
      <label className="log-filter-control">
        <span>{t("details.logFilterLabel")}</span>
        <select name="logFilter" className="log-filter-select" value={filter} onChange={(event) => action("logFilter", event.target.value)}>
          {FILTERS.map((option) => <option key={option.id} value={option.id}>{t(option.label)}</option>)}
          {log.speakers.map((speaker) => <option key={speaker.tag} value={speaker.tag} title={speaker.tag}>{speakerDisplayName(speaker.tag, speaker.name)}</option>)}
        </select>
      </label>
      <div className="log-delete-control">
        <span className="field-help">{t("details.logDeleteSelectedDate", { date: log.selectedDate ?? "—" })}</span>
        <button
          type="button"
          disabled={log.selectedDate === null || log.loading || log.deleting}
          onClick={() => action("logDeleteRequest")}
        >{t("details.logDelete")}</button>
      </div>
    </div>
    {details === null && log.speakerEditor == null ? null : <aside ref={speakerAsideRef} className="log-speaker-details">
    <h3>{t("details.speakerDetailsTitle")}</h3>
    {details === null ? null : <SpeakerDecisionDetails details={details} />}
    {log.speakerEditor == null ? null : <form className="log-speaker-editor" onSubmit={(event) => {
      event.preventDefault();
      action("logSpeakerSave");
    }}>
      <div className="log-speaker-editor-heading">
        <h3>{t("details.logSpeakerEditorTitle")}</h3>
        <span title={log.speakerEditor.speakerId}>{speakerDisplayName(log.speakerEditor.speakerId, null)}</span>
      </div>
      <label className="log-speaker-editor-field">
        <span>{t("details.logSpeakerName")}</span>
        <input
          value={log.speakerEditor.name}
          disabled={log.speakerEditor.saving}
          autoFocus
          onChange={(event) => action("logSpeakerName", event.target.value)}
        />
      </label>
      <div className="log-speaker-editor-actions">
        <button type="submit" disabled={log.speakerEditor.saving}>{t("details.logSpeakerSave")}</button>
        <button type="button" disabled={log.speakerEditor.saving} onClick={() => action("logSpeakerClear")}>{t("details.logSpeakerClear")}</button>
      </div>
      {log.speakerEditor.error === null ? null : <p className="dataflow-empty" role="alert">{log.speakerEditor.error}</p>}
    </form>}
    </aside>}
    {log.loadError === null ? null : <p className="dataflow-empty" role="alert">{log.loadError}</p>}
    {log.deleteError === null ? null : <p className="dataflow-empty" role="alert">{log.deleteError}</p>}
    {log.truncated ? <p className="field-help" role="note">{t("details.logTruncated")}</p> : null}
    <div
      className="dataflow-list log-list"
      ref={listRef}
      onScroll={(event) => {
        const list = event.currentTarget;
        scrollGeneration.current += 1;
        action("logScroll", {
          distance: list.scrollHeight - list.scrollTop - list.clientHeight,
          generation: scrollGeneration.current,
        });
      }}
    >
      {log.entries.length === 0 && !log.loading ? <p className="dataflow-empty">{t("details.logEmpty")}</p> : null}
      {log.entries.map((entry) => <LogRow key={rowKey(entry)} entry={entry} action={action} />)}
    </div>
  </div>;
}
