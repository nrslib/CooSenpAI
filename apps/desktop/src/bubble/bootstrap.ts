import type { BubbleSnapshot, IpcResult } from "../types.js";

export interface BubbleSubscription {
  readonly dispose: () => void;
  readonly ready: Promise<void>;
}

export interface BubbleRendererApi {
  subscribeShow(listener: (snapshot: BubbleSnapshot) => void): BubbleSubscription;
  rendererReady(attempt: number): Promise<IpcResult<BubbleSnapshot>>;
}

export type RetryScheduler = (callback: () => void, delayMs: number) => () => void;

const browserScheduler: RetryScheduler = (callback, delayMs) => {
  const timer = window.setTimeout(callback, delayMs);
  return () => window.clearTimeout(timer);
};

export function rendererRetryDelay(attempt: number): number {
  return Math.min(1_000, 100 * (2 ** Math.max(0, attempt - 1)));
}

export function connectBubbleRenderer(
  api: BubbleRendererApi,
  apply: (snapshot: BubbleSnapshot) => void,
  schedule: RetryScheduler = browserScheduler,
): () => void {
  let disposed = false;
  let attempt = 0;
  let subscription: BubbleSubscription | undefined;
  let cancelRetry: (() => void) | undefined;

  const retry = (): void => {
    if (disposed) return;
    cancelRetry?.();
    cancelRetry = schedule(connect, rendererRetryDelay(attempt));
  };

  const connect = (): void => {
    if (disposed) return;
    attempt += 1;
    let candidate: BubbleSubscription;
    try {
      candidate = api.subscribeShow(apply);
    } catch {
      retry();
      return;
    }
    void candidate.ready.then(async () => {
      if (disposed) {
        candidate.dispose();
        return;
      }
      const snapshot = await api.rendererReady(attempt);
      if (disposed) {
        candidate.dispose();
        return;
      }
      if (!snapshot.ok) {
        candidate.dispose();
        retry();
        return;
      }
      subscription?.dispose();
      subscription = candidate;
      apply(snapshot.value);
    }).catch(() => {
      candidate.dispose();
      retry();
    });
  };

  connect();
  return () => {
    disposed = true;
    cancelRetry?.();
    subscription?.dispose();
  };
}
