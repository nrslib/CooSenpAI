export type WorkKind = "investigate" | "work";
export type WorkRootStatus =
  | { readonly status: "inside"; readonly root: string }
  | { readonly status: "outside" };

export interface WorkSnapshot {
  readonly error?: string | null;
  readonly task: {
    readonly id: string;
    readonly phase: "running" | "succeeded" | "failed" | "cancelled" | "interrupted";
    readonly request: {
      readonly kind: WorkKind;
      readonly cwd: string;
      readonly brief: { readonly currentInputs: readonly string[]; readonly priorUserMessages: readonly string[]; readonly proposal: string };
    };
    readonly harness: "claude" | "codex";
    readonly allowedRoots: readonly { readonly path: string; readonly read: boolean; readonly write: boolean }[];
    readonly resolvedCwd: string | null;
    readonly root: WorkRootStatus | null;
    readonly answer: string | null;
    readonly changedFiles: readonly string[];
    readonly stderrSummary: string | null;
    readonly error: string | null;
  } | null;
  readonly approval: {
    readonly id: string;
    readonly taskId: string;
    readonly kind: WorkKind;
    readonly target: string;
    readonly root: WorkRootStatus;
    readonly reason: string;
    readonly status: "awaitingUser" | "reviewing" | "approved" | "denied" | "cancelled";
    readonly decisionReason: string | null;
  } | null;
}
