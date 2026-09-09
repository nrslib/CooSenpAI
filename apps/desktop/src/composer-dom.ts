import type { ComposerInput, ComposerKeyBinding, ComposerSelection, ComposerSelectionState, ComposerView } from "./composer-view.js";

interface SelectionRequest {
  readonly directive: number;
  readonly revision: number;
  readonly selection: ComposerSelection;
  readonly focus: boolean;
}
export interface ComposerDomState {
  readonly revision: number;
  readonly directive: number;
  readonly message: string;
  readonly selectionState: ComposerSelectionState;
  readonly request: SelectionRequest | null;
  readonly ime: { readonly session: number; readonly active: boolean };
}
interface DomValue { readonly text: string; readonly selection: ComposerSelection }
export type ComposerDomEvent =
  | { readonly type: "render"; readonly view: ComposerView }
  | { readonly type: "frame"; readonly directive: number; readonly revision: number }
  | ({ readonly type: "edit" | "submit" } & DomValue)
  | { readonly type: "selection"; readonly selection: ComposerSelection }
  | ({ readonly type: "composition"; readonly active: boolean } & DomValue)
  | ({ readonly type: "key"; readonly composing: boolean; readonly keyCode: number } & ComposerKeyBinding & DomValue);
interface Result {
  readonly state: ComposerDomState;
  readonly input?: ComposerInput;
  readonly apply?: SelectionRequest;
  readonly body?: string;
}

export function initialComposerDom(view: ComposerView): ComposerDomState {
  return {
    revision: view.editRevision, directive: view.directive, message: view.text,
    selectionState: "pending",
    request: { directive: view.directive, revision: view.editRevision, selection: view.selection, focus: view.focus },
    ime: view.ime,
  };
}

export function transitionComposerDom(state: ComposerDomState, event: ComposerDomEvent): Result {
  switch (event.type) {
    case "render": {
      const view = event.view;
      if (view.editRevision < state.revision || view.directive < state.directive) return { state };
      const replacement = view.directive > state.directive;
      const acknowledged = view.editRevision > state.revision ? cancelSelection(state) : state;
      return { body: view.text, state: {
        ...state, revision: view.editRevision, directive: view.directive, message: view.text,
        ime: state.ime.active ? state.ime : { session: Math.max(state.ime.session, view.ime.session), active: false },
        selectionState: replacement ? "pending" : acknowledged.selectionState,
        request: replacement ? { directive: view.directive, revision: view.editRevision, selection: view.selection, focus: view.focus } : acknowledged.request,
      } };
    }
    case "frame": {
      const request = state.request;
      if (request === null || state.selectionState !== "pending"
        || request.directive !== event.directive || request.revision !== event.revision
        || state.revision !== event.revision) return { state };
      return { state: { ...state, selectionState: "applied", request: null }, apply: request };
    }
    case "edit": {
      const next = cancelSelection({ ...state, revision: state.revision + 1, message: event.text });
      return { state: next, input: { type: "edit", ...draft(next, event) } };
    }
    case "composition": {
      const ime = event.active
        ? { session: state.ime.session + Number(!state.ime.active), active: true }
        : { ...state.ime, active: false };
      const next = event.active ? cancelSelection({ ...state, ime }) : { ...state, ime };
      return { state: next, input: { type: "composition", session: ime.session, active: event.active, ...draft(next, event) } };
    }
    case "selection":
      return { state, input: { type: "selection", editRevision: state.revision, directive: state.directive, selectionState: state.selectionState, selection: event.selection } };
    case "submit":
      return { state, input: { type: "submit", ...draft(state, event) } };
    case "key":
      return { state, input: { ...event, ...draft(state, event) } };
  }
}

function cancelSelection(state: ComposerDomState): ComposerDomState {
  return state.request === null ? state : { ...state, request: null, selectionState: "cancelled" };
}
function draft(state: ComposerDomState, value: DomValue) {
  return { revision: state.revision, directive: state.directive, selectionState: state.selectionState, text: value.text, selection: value.selection };
}
