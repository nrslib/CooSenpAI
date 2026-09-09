import { useEffect, useRef, type ReactElement } from "react";
import type { VRM } from "@pixiv/three-vrm";
import { readVrmImageQuality, readVrmModel } from "../vrm/model-store.js";
import { disposeVrmModel, loadVrmModel, mountVrmScene, type VrmSceneHandle } from "../vrm/runtime.js";
import { loadAssignedMotions } from "../vrm/motion-settings.js";
import { localizeVrmError } from "../vrm/errors.js";
import type { AvatarSceneInput, AvatarSceneView } from "./scene-view.js";
import { useI18n } from "../i18n/index.js";

export function AvatarScene({ view, onEvent }: { readonly view: AvatarSceneView; readonly onEvent: (event: AvatarSceneInput) => void }): ReactElement {
  const { locale, t } = useI18n();
  const host = useRef<HTMLDivElement>(null);
  const scene = useRef<VrmSceneHandle | undefined>(undefined);
  const current = useRef({ view, onEvent, locale }); current.current = { view, onEvent, locale };
  useEffect(() => {
    if (!view.load || host.current === null) return;
    const element = host.current; const controller = new AbortController();
    let model: VRM | undefined; let mounted: VrmSceneHandle | undefined;
    const report = (event: AvatarSceneInput) => current.current.onEvent(event);
    const release = () => { mounted?.dispose(); mounted = undefined; if (model !== undefined) { disposeVrmModel(model); model = undefined; } scene.current = undefined; };
    void (async () => {
      try {
        const [stored, quality] = await Promise.all([readVrmModel(), readVrmImageQuality()]);
        controller.signal.throwIfAborted();
        if (stored === undefined) { report({ type: "loaded", generation: view.generation, outcome: "unset" }); return; }
        const candidate = await loadVrmModel(stored, controller.signal, quality);
        if (controller.signal.aborted) { disposeVrmModel(candidate); return; }
        model = candidate;
        const motions = await loadAssignedMotions(controller.signal); controller.signal.throwIfAborted();
        const state = current.current.view;
        mounted = mountVrmScene(element, model, state.emotions, state.snapshot.thinking, (cause) => {
          if (!controller.signal.aborted) report({ type: "failed", generation: view.generation, error: localizeVrmError(cause, current.current.locale) });
        }, motions);
        scene.current = mounted;
        report({ type: "loaded", generation: view.generation, outcome: "ready" });
      } catch (cause) {
        if (controller.signal.aborted) return;
        release(); report({ type: "failed", generation: view.generation, error: localizeVrmError(cause, current.current.locale) });
      }
    })();
    return () => { controller.abort(); release(); };
  }, [view.generation, view.load]);
  useEffect(() => { scene.current?.update(view.emotions, view.snapshot.thinking); }, [view.emotions, view.snapshot.thinking]);
  useEffect(() => { if (view.replyAction === "reply") scene.current?.reply(); else if (view.replyAction === "cancel") scene.current?.cancelReply(); }, [view.replyDirective]);
  useEffect(() => {
    if (view.preview === null) return;
    if (scene.current === undefined) { onEvent({ type: "failed", generation: view.generation, error: t("vrm.errors.motion.avatarNotReady") }); return; }
    scene.current.preview(view.preview.slot);
    onEvent({ type: "previewApplied", generation: view.generation, requestId: view.preview.id });
  }, [view.previewDirective]);
  return <div className="avatar-stage" ref={host} role="img" aria-label={view.snapshot.thinking ? t("vrm.thinkingLabel") : t("vrm.label")} />;
}
