import { type ReactElement } from "react";

import type { ConversationGeneration } from "../types.js";
import { useI18n, type Locale } from "../i18n/index.js";

interface Props {
  readonly generations: readonly ConversationGeneration[];
  readonly selectedGeneration: number;
  readonly switching: boolean;
  readonly error?: string;
  readonly onSelect: (generation: number) => void;
}

export function ConversationsPanel({ generations, selectedGeneration, switching, error, onSelect }: Props): ReactElement {
  const { locale, t } = useI18n();
  if (generations.length === 0) return <p className="details-conversations-empty">{t("details.conversationEmpty")}</p>;
  return <div className="details-conversations">
    <ul className="details-conversation-list">
      {generations.map((item) => {
        const current = item.generation === selectedGeneration;
        return <li key={item.generation}>
          <button
            type="button"
            className="details-conversation-row"
            aria-current={current ? "true" : undefined}
            disabled={switching}
            onClick={() => onSelect(item.generation)}
          >
            <span className="details-conversation-number">#{item.generation}</span>
            <span className="details-conversation-started">{startedLabel(item, locale, t)}</span>
            <span className="details-conversation-first">{item.firstMessage === undefined || item.firstMessage.length === 0 ? t("details.conversationNoMessage") : item.firstMessage}</span>
            <span className="details-conversation-count">{t("details.conversationEntries", { count: item.entryCount })}</span>
            {current ? <span className="details-conversation-current">{t("details.conversationCurrent")}</span> : null}
          </button>
        </li>;
      })}
    </ul>
    {switching ? <p role="status">{t("details.conversationSwitching")}</p> : null}
    {error === undefined ? null : <p role="alert">{error}</p>}
  </div>;
}

function startedLabel(item: ConversationGeneration, locale: Locale, translate: ReturnType<typeof useI18n>["t"]): string {
  if (item.startedAt === undefined) return translate("details.conversationUnknownDate");
  const date = new Date(item.startedAt);
  if (Number.isNaN(date.getTime())) return translate("details.conversationUnknownDate");
  return new Intl.DateTimeFormat(locale === "ja" ? "ja-JP" : "en-US", {
    year: "numeric",
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}
