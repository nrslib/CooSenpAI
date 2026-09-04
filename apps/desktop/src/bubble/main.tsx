import { StrictMode, useEffect, useLayoutEffect, useRef, useState, type ReactElement } from "react";
import { createRoot } from "react-dom/client";

import { Bubble } from "./Bubble.js";
import { bubbleApi } from "../ipc.js";
import type { BubbleRecord, BubbleSnapshot } from "../types.js";
import { classifyBubbleUpdate, reuseUnchangedBubbleRecords, stableBubbleOrder } from "./state.js";
import { connectBubbleRenderer } from "./bootstrap.js";
import { interactionFailure } from "./interaction.js";
import { applyAppearance } from "../appearance.js";
import "./styles.css";

const EXIT_DURATION_MS = 180;

interface PresentedBubble {
  readonly record: BubbleRecord;
  readonly exiting: boolean;
}

function BubbleApp(): ReactElement {
  const [presented, setPresented] = useState<readonly PresentedBubble[]>([]);
  const [avatarColor, setAvatarColor] = useState<string>();
  const [avatarImage, setAvatarImage] = useState<readonly number[]>();
  const [position, setPosition] = useState<BubbleSnapshot["position"]>("bottom-right");
  const [pendingAck, setPendingAck] = useState<{ readonly generation: number; readonly sequence: number }>();
  const stackRef = useRef<HTMLDivElement>(null);
  const generationRef = useRef(-1);
  const recordsRef = useRef<readonly PresentedBubble[]>([]);
  const liveIdsRef = useRef<ReadonlySet<string>>(new Set());
  const exitTimers = useRef(new Map<string, number>());
  const layoutQueueRef = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    const present = (records: readonly BubbleRecord[]): void => {
      const previous = recordsRef.current
        .filter((item) => !item.exiting)
        .map((item) => item.record);
      const ordered = reuseUnchangedBubbleRecords(previous, stableBubbleOrder(records));
      const liveIds = new Set(ordered.map((record) => record.id));
      liveIdsRef.current = liveIds;
      for (const record of ordered) {
        const timer = exitTimers.current.get(record.id);
        if (timer !== undefined) window.clearTimeout(timer);
        exitTimers.current.delete(record.id);
      }
      const leaving = recordsRef.current
        .filter((item) => !item.exiting && !liveIds.has(item.record.id))
        .map((item) => ({ ...item, exiting: true }));
      const next = [
        ...ordered.map((record) => ({ record, exiting: false })),
        ...leaving,
      ];
      recordsRef.current = next;
      setPresented(next);
      for (const item of leaving) {
        if (exitTimers.current.has(item.record.id)) continue;
        const timer = window.setTimeout(() => {
          exitTimers.current.delete(item.record.id);
          if (liveIdsRef.current.has(item.record.id)) return;
          const remaining = recordsRef.current.filter((entry) => entry.record.id !== item.record.id);
          recordsRef.current = remaining;
          setPresented(remaining);
        }, EXIT_DURATION_MS);
        exitTimers.current.set(item.record.id, timer);
      }
    };
    const apply = (snapshot: BubbleSnapshot): void => {
      const decision = classifyBubbleUpdate(generationRef.current, snapshot);
      if (decision === "ignore") return;
      applyAppearance({ theme: snapshot.theme, font: snapshot.font });
      setAvatarColor(snapshot.avatarColor);
      setAvatarImage(snapshot.avatarImagePng);
      setPosition(snapshot.position);
      if (decision === "apply") {
        generationRef.current = snapshot.generation;
        present(snapshot.records);
      }
      setPendingAck((current) => ({
        generation: snapshot.generation,
        sequence: (current?.sequence ?? 0) + 1,
      }));
    };
    const disconnect = connectBubbleRenderer(bubbleApi, apply);
    const timers = exitTimers.current;
    return () => {
      disconnect();
      for (const timer of timers.values()) window.clearTimeout(timer);
      timers.clear();
    };
  }, []);

  useLayoutEffect(() => {
    const stack = stackRef.current;
    if (stack === null) return;
    const enqueueLayout = (ackGeneration?: number): void => {
      const height = Math.min(680, Math.max(96, Math.ceil(stack.scrollHeight)));
      layoutQueueRef.current = layoutQueueRef.current.then(async () => {
        if (presented.length > 0) await bubbleApi.resize(height);
        if (ackGeneration !== undefined) await bubbleApi.ack(ackGeneration);
      });
    };
    enqueueLayout(pendingAck?.generation);
    if (presented.length === 0) return;
    const observer = new ResizeObserver(() => enqueueLayout());
    observer.observe(stack);
    return () => observer.disconnect();
  }, [pendingAck, presented, position]);

  return <div
    ref={stackRef}
    className={`bubble-stack position-${position}`}
    onClick={(event) => { if (event.target === event.currentTarget) void bubbleApi.fastForward(); }}
    onPointerMove={(event) => { if (event.target === event.currentTarget) void bubbleApi.passThrough(); }}
  >
    {presented.map(({ record, exiting }) => <Bubble
      key={record.id}
      record={record}
      avatarColor={avatarColor}
      avatarImage={avatarImage}
      exiting={exiting}
      onHover={(hovering) => { void bubbleApi.hover(record.id, hovering); }}
      onRequestFocus={() => { void bubbleApi.focus(); }}
      onDismiss={() => { void bubbleApi.dismiss(record.id); }}
      onClick={() => { void bubbleApi.fastForward(record.id).then((result) => { if (!result.ok || !result.value) void bubbleApi.click(record.id); }); }}
      onInteract={async (action, value) => {
        return interactionFailure(await bubbleApi.interact(record.id, action, value));
      }}
    />)}
  </div>;
}

const root = document.getElementById("root");
if (root === null) throw new Error("吹き出しのroot要素が見つかりません。");
createRoot(root).render(<StrictMode><BubbleApp /></StrictMode>);
