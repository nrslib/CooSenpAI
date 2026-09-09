import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createInterface } from "node:readline";
import type { AppSnapshot, BubbleSnapshot, IpcResult } from "../types.js";

export interface TutorialBackendState {
  readonly id: string;
  readonly snapshot: AppSnapshot;
  readonly bubbles: BubbleSnapshot;
  readonly onboarding: { readonly tutorial: { readonly steps: { readonly chat: { readonly status: string } }; readonly notices: Readonly<Record<string, { readonly bubbleAccepted: boolean }>> } };
  readonly mainVisible: boolean;
  readonly pointerEnabled: boolean;
  readonly renders: readonly { readonly channel: string; readonly payload: unknown }[];
  readonly events: readonly { readonly channel: string; readonly payload: unknown; readonly locksHeld: boolean }[];
}

export interface TutorialClickResponse {
  readonly result: IpcResult<null>;
  readonly state: TutorialBackendState;
}

// WebView の輸送だけを差し替え、Rust の共通 Root と Presenter を stdio 経由で呼ぶ。
export class TutorialIpcBridge {
  private process: ChildProcessWithoutNullStreams | undefined;
  private sequence = 0;
  private readonly pending = new Map<number, { readonly resolve: (value: unknown) => void; readonly reject: (error: Error) => void }>();
  private exited: Promise<void> = Promise.resolve();

  async start(): Promise<void> {
    const child = spawn("cargo", [
      "test", "--offline", "-q", "-p", "coosenpai-desktop", "--lib",
      "bubble_click_tests::renderer_ipc_bridge", "--", "--exact", "--ignored", "--nocapture",
    ], { stdio: "pipe" });
    this.process = child;
    let stderr = "";
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
    this.exited = new Promise((resolve) => child.once("exit", () => resolve()));
    await new Promise<void>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) => {
        const error = new Error(`Rust IPC bridge exited (${code}): ${stderr}`);
        reject(error);
        for (const request of this.pending.values()) request.reject(error);
        this.pending.clear();
      });
      createInterface({ input: child.stdout }).on("line", (line) => {
        if (!line.startsWith("TUTORIAL_IPC ")) return;
        const message = JSON.parse(line.slice("TUTORIAL_IPC ".length)) as { ready?: boolean; sequence?: number; value?: unknown };
        if (message.ready) resolve();
        if (message.sequence !== undefined) {
          this.pending.get(message.sequence)?.resolve(message.value);
          this.pending.delete(message.sequence);
        }
      });
    });
  }

  request<T>(request: Readonly<Record<string, unknown>>): Promise<T> {
    const child = this.process;
    if (child === undefined || child.exitCode !== null) return Promise.reject(new Error("Rust IPC bridge is not running"));
    const sequence = ++this.sequence;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(sequence, { resolve: (value) => resolve(value as T), reject });
      child.stdin.write(`${JSON.stringify({ ...request, sequence })}\n`);
    });
  }

  async close(): Promise<void> {
    if (this.process === undefined || this.process.exitCode !== null) return;
    this.process.stdin.end(`${JSON.stringify({ operation: "close" })}\n`);
    await this.exited;
  }
}
