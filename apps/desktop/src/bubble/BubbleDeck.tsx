import { useRef, type ReactElement } from "react";

import type { BubbleSnapshot } from "../types.js";
import { useI18n } from "../i18n/index.js";
import type { BubbleControlsView, BubbleViewInput } from "./view.js";
import { Bubble } from "./Bubble.js";

interface Props {
  readonly snapshot: BubbleSnapshot;
  readonly controls: BubbleControlsView;
  readonly typing?: { id: string; revealed: number | null };
  readonly onHover: (id: string, hovering: boolean) => void;
  readonly onEvent: (event: BubbleViewInput) => void;
}

export function BubbleDeck({ snapshot, controls, typing, onHover, onEvent }: Props): ReactElement {
  const faceRef = useRef<HTMLDivElement>(null);
  const { t } = useI18n();
  const record = controls.record;
  return <section className="bubble-deck" aria-label={t("bubble.cards")}
    onWheel={(event) => {
      const face = faceRef.current;
      if (face === null) return;
      onEvent({ type: "wheel", delta: event.deltaY, control: event.target instanceof HTMLElement && event.target.closest("select,input,textarea") !== null,
        scrollTop: face.scrollTop, clientHeight: face.clientHeight, scrollHeight: face.scrollHeight });
    }}>
    {controls.olderEdge ? <button className={`deck-edge${controls.secondEdge ? " has-second-edge" : ""}`} type="button" aria-label={t("bubble.previousCard")} disabled={controls.navigationDisabled} onClick={() => onEvent({ type: "navigate", direction: "older" })} /> : null}
    <div className="deck-face" ref={faceRef}>
      {record === null ? null : <Bubble key={record.id} record={record} controls={controls}
        revealed={typing?.id === record.id ? typing.revealed : null} avatarColor={snapshot.avatarColor} avatarImage={snapshot.avatarImagePng}
        exiting={controls.exiting} reading={controls.reading} onHover={(hovering) => onHover(record.id, hovering)} onEvent={onEvent} />}
    </div>
    {controls.showLatest ? <button className="deck-latest" type="button" onClick={() => onEvent({ type: "navigate", direction: "latest" })}>{t("bubble.latestCard")}</button> : null}
  </section>;
}
