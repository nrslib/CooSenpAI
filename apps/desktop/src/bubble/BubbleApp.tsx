import { useEffect, useLayoutEffect, useRef, useState, type ReactElement } from "react";

import { bubbleApi, setIpcLocale } from "../ipc.js";
import type { BubbleSnapshot } from "../types.js";
import { classifyBubbleUpdate } from "./state.js";
import { connectBubbleRenderer } from "./bootstrap.js";
import type { BubbleControlsView } from "./view.js";
import { applyAppearance } from "../appearance.js";
import { I18nProvider } from "../i18n/index.js";
import { BubbleDeck } from "./BubbleDeck.js";

export function BubbleApp(): ReactElement {
  const [controls, setControls] = useState<BubbleControlsView>();
  const [typing, setTyping] = useState<{ id: string; revealed: number | null }>();
  const [snapshot, setSnapshot] = useState<BubbleSnapshot>();
  const [pendingAck, setPendingAck] = useState<{ readonly generation: number; readonly sequence: number }>();
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const stackRef = useRef<HTMLDivElement>(null);
  const generationRef = useRef(-1);
  const layoutQueueRef = useRef<Promise<void>>(Promise.resolve());
  const lastPointRef = useRef<{ x: number; y: number } | null>(null);

  const updateHover = (point: { x: number; y: number } | null): boolean => {
    lastPointRef.current = point;
    const stack = stackRef.current;
    const article = stackRef.current?.querySelector<HTMLElement>(".speech-item") ?? null;
    const record = controls?.record;
    const overCard = point !== null && stack !== null && article !== null && !controls?.exiting &&
      [".speech-bubble", ".speaker", ".bubble-close-lane"].some((selector) => {
        const element = article.querySelector(selector);
        if (element === null) return false;
        return containsVisiblePoint(element, stack, point);
      });
    const id = overCard && record !== undefined && record !== null ? record.id : null;
    setHoveredId(id);
    return overCard;
  };

  useEffect(() => {
    const receive = (event: Event): void => {
      const point = (event as CustomEvent<{ x: number; y: number } | null>).detail;
      const overCard = updateHover(point);
      if (point !== null && !overCard && ![".deck-edge", ".deck-latest"].some((selector) => {
        const element = stackRef.current?.querySelector(selector);
        return element !== null && element !== undefined && stackRef.current !== null && containsVisiblePoint(element, stackRef.current, point);
      })) void bubbleApi.passThrough();
    };
    window.addEventListener("coosenpai:bubble:pointer-position", receive);
    updateHover(lastPointRef.current);
    return () => window.removeEventListener("coosenpai:bubble:pointer-position", receive);
  }, [controls, snapshot]);

  useEffect(() => {
    if (hoveredId === null) return;
    void bubbleApi.hover(hoveredId, true);
    return () => { void bubbleApi.hover(hoveredId, false); };
  }, [hoveredId]);

  useEffect(() => {
    const controls = bubbleApi.subscribeControls(setControls);
    const typing = bubbleApi.subscribeTyping(setTyping);
    const clear = bubbleApi.subscribeClear(() => setSnapshot(undefined));
    const disconnect = connectBubbleRenderer(bubbleApi, (incoming) => {
    if (classifyBubbleUpdate(generationRef.current, incoming) === "ignore") return;
    applyAppearance({ theme: incoming.theme, font: incoming.font });
    setIpcLocale(incoming.language);
    generationRef.current = incoming.generation;
    setSnapshot(incoming);
    setPendingAck((current) => ({ generation: incoming.generation, sequence: (current?.sequence ?? 0) + 1 }));
    }, [typing.ready, clear.ready, controls.ready]);
    return () => { disconnect(); typing.dispose(); clear.dispose(); controls.dispose(); };
  }, []);

  useLayoutEffect(() => {
    const stack = stackRef.current;
    if (stack === null || snapshot === undefined) return;
    const enqueueLayout = (ackGeneration?: number): void => {
      const height = Math.min(680, Math.max(96, Math.ceil(stack.scrollHeight)));
      layoutQueueRef.current = layoutQueueRef.current.then(async () => {
        if (snapshot.frontId !== null) await bubbleApi.resize(height);
        if (ackGeneration !== undefined) await bubbleApi.ack(ackGeneration);
      });
    };
    enqueueLayout(pendingAck?.generation);
    const observer = new ResizeObserver(() => enqueueLayout());
    observer.observe(stack);
    return () => observer.disconnect();
  }, [pendingAck, snapshot]);

  if (snapshot === undefined) return <div />;
  return <I18nProvider locale={snapshot.language}><div
    ref={stackRef}
    className={`bubble-stack position-${snapshot.position}`}
    onClick={(event) => { if (event.target === event.currentTarget) void bubbleApi.fastForward(); }}
    onPointerMove={(event) => {
      updateHover({ x: event.clientX, y: event.clientY });
      if (!(event.target instanceof Element) || event.target.closest(".bubble-surface,.bubble-close-lane,.speaker,.deck-edge,.deck-latest") === null) void bubbleApi.passThrough();
    }}
    onPointerLeave={() => updateHover(null)}
  >
    {controls === undefined ? null : <BubbleDeck snapshot={snapshot} controls={controls} typing={typing} hoveredId={hoveredId}
      onEvent={(event) => {
        void bubbleApi.input(event).then((result) => {
          if (!result.ok) console.error("Bubble input IPC error", event.type, result.error.message);
        });
      }} />}

  </div></I18nProvider>;
}

function containsPoint(element: Element, point: { x: number; y: number }): boolean {
  const rect = element.getBoundingClientRect();
  return point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom;
}

function containsVisiblePoint(element: Element, stack: Element, point: { x: number; y: number }): boolean {
  const face = element.closest(".deck-face");
  return containsPoint(element, point) && containsPoint(stack, point) && (face === null || containsPoint(face, point));
}
