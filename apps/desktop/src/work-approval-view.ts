import type { WorkSnapshot } from "./work.js";
export type WorkAction = "allow" | "deny" | "addRootAndAllow" | "toggleMode" | "cancel";
export type WorkApprovalInput = { readonly type: "mounted" | "unmounted"; readonly inputId: string } | { readonly type: "action"; readonly inputId: string; readonly approvalId: string | null; readonly action: WorkAction };
export interface WorkApprovalView {
  readonly inputId: string | null; readonly visible: boolean; readonly approval: WorkSnapshot["approval"];
  readonly awaiting: boolean; readonly busy: boolean; readonly manual: boolean; readonly error: string | null; readonly layoutRequest: number;
}
