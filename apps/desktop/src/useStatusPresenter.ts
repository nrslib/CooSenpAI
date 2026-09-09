import { useEffect, useRef } from "react";
import type { IpcResult } from "./types.js";
import { desktopApi, ipcTransportError } from "./ipc.js";
import { usePanelPresenter, type PanelCommand } from "./usePanelPresenter.js";
import type { UpdateSnapshot } from "./app-update.js";
import type { VoiceOutputSnapshot } from "./voice-output.js";

export interface StatusView<S> {
  readonly snapshot: S | null;
  readonly error: string | null;
  readonly busy: boolean;
  readonly visible: boolean;
  readonly canCheck: boolean;
  readonly canInstall: boolean;
  readonly canRestart: boolean;
  readonly canTest: boolean;
  readonly canStop: boolean;
  readonly showCheck: boolean;
  readonly showInstall: boolean;
  readonly showRestart: boolean;
  readonly showLater: boolean;
}

export function useStatusPresenter<S extends UpdateSnapshot | VoiceOutputSnapshot>(kind: "update" | "voice", enabled: boolean, showControls: boolean) {
  const subscribed = useRef<Promise<IpcResult<null>>>(Promise.resolve({ ok: true, value: null }));
  const presenter = usePanelPresenter<StatusView<S>, PanelCommand>(kind, { enabled, showControls }, async (command) => {
    switch (command.kind) {
      case "load": {
        const ready = await subscribed.current;
        if (!ready.ok) return ready;
        return kind === "update" ? desktopApi.getAppUpdateStatus() : desktopApi.getVoiceOutputStatus();
      }
      case "check": return desktopApi.checkAppUpdate();
      case "install": return desktopApi.installAppUpdate();
      case "restart": return desktopApi.relaunch();
      case "test": return desktopApi.testVoiceOutput();
      case "stop": return desktopApi.stopVoiceOutput();
      default: throw new Error(`Unknown status command: ${command.kind}`);
    }
  });
  useEffect(() => {
    const observed = (value: UpdateSnapshot | VoiceOutputSnapshot): void => presenter.send({ type: "action", name: "observed", value });
    const subscription = kind === "update" ? desktopApi.subscribeAppUpdates(observed) : desktopApi.subscribeVoiceOutput(observed);
    subscribed.current = subscription.ready.then(() => ({ ok: true as const, value: null }), (cause: unknown) => ({ ok: false as const, error: { message: ipcTransportError(cause) } }));
    return () => subscription.dispose();
  }, [kind, presenter.send]);
  useEffect(() => presenter.send({ type: "change", value: { enabled, showControls } }), [enabled, showControls, presenter.send]);
  return { ...presenter, action: (name: string): void => presenter.send({ type: "action", name, value: null }) };
}
