export interface ModelPickerView {
  readonly provider: string;
  readonly model: string;
  readonly effort: string;
  readonly providers: readonly string[];
  readonly models: readonly string[];
  readonly efforts: readonly string[];
  readonly disabled: boolean;
  readonly reloading: boolean;
  readonly showReload: boolean;
  readonly showClaudeHelp: boolean;
  readonly opencodeFailed: boolean;
  readonly notice: boolean;
  readonly error: string | null;
}

export type ModelPickerInput =
  | { readonly type: "provider" | "model" | "effort"; readonly value: string }
  | { readonly type: "commitModel" | "commitEffort" | "reload" };
