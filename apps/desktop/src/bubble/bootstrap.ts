import type { BubbleSnapshot, IpcResult } from "../types.js";

export interface BubbleSubscription {
  readonly dispose: () => void;
  readonly ready: Promise<void>;
}

export interface BubbleRendererApi {
  subscribeShow(listener: (snapshot: BubbleSnapshot) => void): BubbleSubscription;
  rendererReady(attempt: number): Promise<IpcResult<null>>;
}

export function connectBubbleRenderer(api: BubbleRendererApi, apply: (snapshot: BubbleSnapshot) => void, ready: readonly Promise<void>[] = []): () => void {
  let active = true;
  const subscription = api.subscribeShow((snapshot) => { if (active) apply(snapshot); });
  void Promise.all([subscription.ready, ...ready]).then(() => { if (active) return api.rendererReady(1); });
  return () => { active = false; subscription.dispose(); };
}
