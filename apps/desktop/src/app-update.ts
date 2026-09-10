export type UpdateStatus =
  | { readonly phase: "idle" | "checking" | "upToDate" }
  | { readonly phase: "available"; readonly version: string; readonly notes: string | null }
  | { readonly phase: "incompatible"; readonly version: string; readonly minimumSystemVersion: string }
  | { readonly phase: "downloading"; readonly version: string; readonly downloaded: number; readonly total: number | null }
  | { readonly phase: "installing" | "installed"; readonly version: string }
  | { readonly phase: "failed"; readonly message: string };

export interface UpdateSnapshot {
  readonly revision: number;
  readonly status: UpdateStatus;
}
