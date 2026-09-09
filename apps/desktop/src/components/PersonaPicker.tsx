import type { KeyboardEvent as ReactKeyboardEvent, ReactElement } from "react";

import { t, useI18n, type Locale, type TranslationKey } from "../i18n/index.js";
import type { PersonaOption } from "../types.js";
import { CloseIcon } from "./LineIcons.js";

export interface PersonaReply {
  readonly scene: string;
  readonly message: string;
}

export interface PersonaDisplayData {
  readonly introduction: string;
  readonly replies: readonly [PersonaReply, PersonaReply, PersonaReply];
}

type PersonaReplyKeys = readonly [TranslationKey, TranslationKey];
interface PersonaDisplayKeys {
  readonly introduction: TranslationKey;
  readonly replies: readonly [PersonaReplyKeys, PersonaReplyKeys, PersonaReplyKeys];
}

const BUILTIN_PERSONA_DISPLAY_DATA: Readonly<Record<string, PersonaDisplayKeys>> = {
  "coo-chan": {
    introduction: "persona.cooChan.introduction",
    replies: [
      ["persona.scenes.testPassed", "persona.cooChan.testPassed"],
      ["persona.scenes.repeatedError", "persona.cooChan.repeatedError"],
      ["persona.scenes.spokenTo", "persona.cooChan.spokenTo"],
    ],
  },
  "coo-kun": {
    introduction: "persona.cooKun.introduction",
    replies: [
      ["persona.scenes.testPassed", "persona.cooKun.testPassed"],
      ["persona.scenes.repeatedError", "persona.cooKun.repeatedError"],
      ["persona.scenes.spokenTo", "persona.cooKun.spokenTo"],
    ],
  },
} as const;

const CUSTOM_PERSONA_DISPLAY_DATA: PersonaDisplayKeys = {
  introduction: "persona.custom.introduction",
  replies: [
    ["persona.scenes.testPassed", "persona.custom.testPassed"],
    ["persona.scenes.repeatedError", "persona.custom.repeatedError"],
    ["persona.scenes.spokenTo", "persona.custom.spokenTo"],
  ],
};

function localizeDisplayData(display: PersonaDisplayKeys, locale: Locale): PersonaDisplayData {
  return {
    introduction: t(locale, display.introduction),
    replies: display.replies.map(([scene, message]) => ({ scene: t(locale, scene), message: t(locale, message) })) as [PersonaReply, PersonaReply, PersonaReply],
  };
}

export function personaDisplayData(option: PersonaOption, locale: Locale = "ja"): PersonaDisplayData {
  if (!option.builtin) return localizeDisplayData(CUSTOM_PERSONA_DISPLAY_DATA, locale);
  const display = BUILTIN_PERSONA_DISPLAY_DATA[option.id];
  if (display === undefined) throw new Error(t(locale, "persona.missingData", { id: option.id }));
  return localizeDisplayData(display, locale);
}

export function personaOptionLabel(option: PersonaOption, locale: Locale = "ja"): string {
  return option.builtin ? option.id : t(locale, "persona.customLabel", { name: option.displayName });
}

export interface PersonaCardProps {
  readonly option: PersonaOption;
  readonly display: PersonaDisplayData;
  readonly selected: boolean;
  readonly disabled: boolean;
  readonly onSelect: (persona: string) => void;
  readonly label?: string;
  readonly selectLabel?: string;
  readonly selectedLabel?: string;
}

export function PersonaCard({ option, display, selected, disabled, onSelect, label: labelOverride, selectLabel: selectLabelOverride, selectedLabel: selectedLabelOverride }: PersonaCardProps): ReactElement {
  const label = labelOverride ?? personaOptionLabel(option);
  const selectLabel = selectLabelOverride ?? t("ja", "persona.select", { name: label });
  const selectedLabel = selectedLabelOverride ?? t("ja", "persona.selected");
  return <button
    className={selected ? "persona-card is-selected" : "persona-card"}
    data-persona-id={option.id}
    type="button"
    aria-label={selectLabel}
    aria-pressed={selected}
    disabled={disabled}
    onClick={() => onSelect(option.id)}
  >
    <span className="persona-card-heading"><strong>{label}</strong>{selected ? <span className="persona-card-selected">{selectedLabel}</span> : null}</span>
    <span className="persona-card-introduction">{display.introduction}</span>
    <span className="persona-card-replies">{display.replies.map((reply) => <span className="persona-reply" key={reply.scene}>
      <span className="persona-reply-scene">{reply.scene}</span>
      <span className="persona-reply-bubble">{reply.message}</span>
    </span>)}</span>
  </button>;
}

interface Props {
  readonly personas: readonly PersonaOption[];
  readonly selectedPersona: string;
  readonly busy: boolean;
  readonly error?: string;
  readonly onSelect: (persona: string) => void;
  readonly onClose: () => void;
}

export function PersonaPicker({ personas, selectedPersona, busy, error, onSelect, onClose }: Props): ReactElement {
  const { locale, t: translate } = useI18n();
  const closeOnEscape = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (event.key !== "Escape" || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  };

  return <div className="dialog-overlay persona-picker-overlay" role="presentation" onKeyDown={closeOnEscape}>
    <section className="persona-picker" role="dialog" aria-modal="true" aria-labelledby="persona-picker-title">
      <header className="persona-picker-heading">
        <div><span>{translate("persona.heading")}</span><h2 id="persona-picker-title">{translate("persona.title")}</h2></div>
        <button className="icon-button" type="button" aria-label={translate("persona.close")} onClick={onClose}><CloseIcon /></button>
      </header>
      <p className="persona-picker-help">{translate("persona.help")}</p>
      {personas.length === 0
        ? <p className="persona-picker-status">{translate("persona.loading")}</p>
        : <div className="persona-picker-cards">{personas.map((option) => <PersonaCard
          key={option.id}
          option={option}
          display={personaDisplayData(option, locale)}
          selected={selectedPersona === option.id}
          disabled={busy}
          label={personaOptionLabel(option, locale)}
          selectLabel={translate("persona.select", { name: personaOptionLabel(option, locale) })}
          selectedLabel={translate("persona.selected")}
          onSelect={onSelect}
        />)}</div>}
      {error === undefined ? null : <p className="persona-picker-error" role="alert">{error}</p>}
    </section>
  </div>;
}
