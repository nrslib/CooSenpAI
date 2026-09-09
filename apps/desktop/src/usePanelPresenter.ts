import { useCallback, useEffect, useRef, useState } from "react";
import { desktopApi, ipcTransportError } from "./ipc.js";
import type { IpcResult } from "./types.js";

export type PanelKind = "settings" | "persona" | "vrm" | "update" | "voice" | "details" | "dataflow" | "attachment" | "history";
export type PanelEvent =
  | { readonly type: "mount" | "change"; readonly value: unknown }
  | { readonly type: "action"; readonly name: string; readonly value: unknown }
  | { readonly type: "completed"; readonly id: number; readonly result: IpcResult<unknown> }
  | { readonly type: "unmount" };
export interface PanelCommand { readonly id: number; readonly kind: string; readonly payload: unknown }
interface PanelProjection { readonly revision: number; readonly state: unknown }
interface PanelUpdate extends PanelProjection { readonly session: string }
export interface PanelOutput<S, C> { readonly revision: number; readonly state: S | null; readonly commands: readonly C[]; readonly updates?: readonly PanelUpdate[] }
const projections = new Map<string, (update: PanelProjection) => void>();
function deliverUpdates(updates: readonly PanelUpdate[] = []): void {
  for (const update of updates) projections.get(update.session)?.(update);
}

// この境界は配送と I/O のみを担当し、完了の採否は Rust に戻す。
export function usePanelPresenter<S, C extends PanelCommand>(kind: PanelKind, initial: unknown,
  execute: (command: C) => Promise<IpcResult<unknown>>,
): { readonly state: S | undefined; readonly error: string | undefined; readonly send: (event: PanelEvent) => void } {
  const [state, setState] = useState<S>();
  const [error, setError] = useState<string>();
  const executeRef = useRef(execute);
  executeRef.current = execute;
  const initialRef = useRef(initial);
  const transport = useRef<((event: PanelEvent) => void) | undefined>(undefined);
  const waiting = useRef<PanelEvent[]>([]);
  const send = useCallback((event: PanelEvent): void => {
    if (transport.current === undefined) waiting.current.push(event);
    else transport.current(event);
  }, []);
  useEffect(() => {
    const session = crypto.randomUUID();
    let active = true;
    let revision = 0;
    const project = (output: PanelProjection): void => {
      if (active && output.state !== null && output.revision > revision) {
        revision = output.revision;
        setState(output.state as S);
      }
    };
    projections.set(session, project);
    const receive = async (event: PanelEvent): Promise<void> => {
      const result = await desktopApi.panelEvent<S, C>({ session, kind, event });
      if (result.ok) deliverUpdates(result.value.updates);
      if (!active) return;
      if (!result.ok) { setError(result.error.message); return; }
      const output = result.value;
      project(output);
      for (const command of output.commands) {
        void Promise.resolve().then(async () => {
          if (!active) return;
          let completion: IpcResult<unknown>;
          try { completion = await executeRef.current(command); }
          catch (cause) { completion = { ok: false, error: { message: ipcTransportError(cause) } }; }
          if (active) await receive({ type: "completed", id: command.id, result: completion });
        });
      }
    };
    const mounted = receive({ type: "mount", value: initialRef.current });
    transport.current = (event) => { void mounted.then(() => { if (active) return receive(event); }); };
    for (const event of waiting.current.splice(0)) transport.current(event);
    return () => {
      active = false;
      projections.delete(session);
      transport.current = undefined;
      void mounted.then(async () => {
        const result = await desktopApi.panelEvent({ session, kind, event: { type: "unmount" } });
        if (result.ok) deliverUpdates(result.value.updates);
      });
    };
  }, [kind]);
  return { state, error, send };
}
