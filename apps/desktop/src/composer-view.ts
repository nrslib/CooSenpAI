export interface ComposerSelection { readonly start: number; readonly end: number }
export interface ComposerKeyBinding { readonly key: string; readonly metaKey: boolean; readonly shiftKey: boolean; readonly ctrlKey: boolean; readonly altKey: boolean }
export type ComposerSelectionState = "pending" | "applied" | "cancelled";
export interface ComposerView {
  readonly ime: { readonly session: number; readonly active: boolean };
  readonly editRevision: number; readonly text: string; readonly directive: number; readonly selection: ComposerSelection; readonly focus: boolean;
  readonly ready: boolean; readonly canSend: boolean; readonly pendingSends: number; readonly recording: boolean; readonly bindings: readonly ComposerKeyBinding[]; readonly error: string | null;
}
interface DraftInput { readonly revision: number; readonly directive: number; readonly selectionState: ComposerSelectionState; readonly text: string; readonly selection: ComposerSelection }
export type ComposerInput =
  | ({ readonly type: "edit" | "submit" } & DraftInput)
  | ({ readonly type: "composition"; readonly session: number; readonly active: boolean } & DraftInput)
  | ({ readonly type: "key"; readonly composing: boolean; readonly keyCode: number } & ComposerKeyBinding & DraftInput)
  | { readonly type: "selection"; readonly editRevision: number; readonly directive: number; readonly selectionState: ComposerSelectionState; readonly selection: ComposerSelection }
  | { readonly type: "microphone"; readonly gesture: "press" | "release" | "cancel" | "click" };
export interface ConversationView { readonly scrollRequest: number; readonly target: "bottom" | "entry-start" | "entry-center" | ""; readonly entryId: string | null; readonly selectedId: string | null; readonly thinking: boolean; readonly workInputId: string | null; readonly operationGeneration: number; readonly actions: Readonly<Record<string, { readonly cancel: boolean; readonly retry: boolean; readonly resend: boolean }>>; readonly busy: boolean; readonly error: string | null }
export type ConversationAction = "cancel" | "retry" | "resend";
export type ConversationInput =
  | { readonly type: "action"; readonly action: ConversationAction; readonly inputId: string; readonly generation: number }
  | { readonly type: "mounted" | "opened" | "layout" }
  | { readonly type: "wheel"; readonly delta: number }
  | { readonly type: "scroll"; readonly previousTop: number; readonly top: number; readonly programmatic: boolean }
  | { readonly type: "escape"; readonly composing: boolean; readonly keyCode: number };
