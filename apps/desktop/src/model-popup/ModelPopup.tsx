import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";

import { AvatarBlob } from "../components/AvatarBlob.js";
import { CompanionModelControls } from "../components/CompanionModelControls.js";
import { CloseIcon } from "../components/LineIcons.js";
import { applyAppearance } from "../appearance.js";
import { modelPopupApi } from "../ipc.js";
import { createCatalogRequestGeneration, mergeOpencodeReloadCatalog } from "../model-catalog.js";
import type { AppSnapshot, CompanionModelCatalog, ConfigPatch, CooSenpaiConfig, IpcResult } from "../types.js";
import { applyModelPopupSnapshot, connectModelPopupSnapshots, initialModelPopupViewState, modelPopupControlsDisabled, modelPopupKeyAction } from "./state.js";

export function ModelPopup(): ReactElement {
  const [viewState, setViewState] = useState(initialModelPopupViewState);
  const [catalog, setCatalog] = useState<CompanionModelCatalog>();
  const [error, setError] = useState<string>();
  const catalogRequestGeneration = useRef(createCatalogRequestGeneration());
  const snapshot = viewState.snapshot;

  const requestCatalog = useCallback(async (
    loadCatalog: () => Promise<IpcResult<CompanionModelCatalog>>,
    mergeCatalog: (current: CompanionModelCatalog | undefined, refreshed: CompanionModelCatalog) => CompanionModelCatalog,
  ): Promise<IpcResult<CompanionModelCatalog> | undefined> => {
    const generation = catalogRequestGeneration.current.begin();
    const result = await loadCatalog();
    if (!catalogRequestGeneration.current.isCurrent(generation)) return undefined;
    if (result.ok) setCatalog((current) => mergeCatalog(current, result.value));
    return result;
  }, []);

  useEffect(() => {
    const apply = (next: AppSnapshot): void => {
      setViewState((current) => applyModelPopupSnapshot(current, next));
      setError(undefined);
    };
    return connectModelPopupSnapshots(modelPopupApi, apply, setError);
  }, []);

  useEffect(() => {
    if (snapshot === undefined) return;
    applyAppearance({ theme: snapshot.config.ui.theme, font: snapshot.config.ui.font });
  }, [snapshot?.config.ui.font, snapshot?.config.ui.theme]);

  useEffect(() => {
    if (snapshot === undefined || snapshot.onboarding.setupRequired || snapshot.onboarding.finishPending) return;
    const loadCatalog = (): void => {
      void requestCatalog(() => modelPopupApi.companionModelCatalog(), (_current, refreshed) => refreshed);
    };
    loadCatalog();
    const timer = window.setInterval(loadCatalog, 24 * 60 * 60 * 1000);
    return () => {
      catalogRequestGeneration.current.invalidate();
      window.clearInterval(timer);
    };
  }, [requestCatalog, snapshot?.config.app.checkForUpdates, snapshot?.onboarding.finishPending, snapshot?.onboarding.setupRequired]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent): void => {
      if (modelPopupKeyAction(event.key) !== "close") return;
      event.preventDefault();
      void modelPopupApi.close();
    };
    document.addEventListener("keydown", closeOnEscape);
    return () => document.removeEventListener("keydown", closeOnEscape);
  }, []);

  const save = async (patch: ConfigPatch): Promise<IpcResult<CooSenpaiConfig>> => {
    const result = await modelPopupApi.updateConfig(patch);
    if (result.ok) {
      setError(undefined);
      await requestCatalog(() => modelPopupApi.companionModelCatalog(), (_current, refreshed) => refreshed);
    } else {
      setError(result.error.message);
    }
    return result;
  };

  const reloadOpencode = async (): Promise<void> => {
    const result = await requestCatalog(
      () => modelPopupApi.reloadOpencodeModels(),
      mergeOpencodeReloadCatalog,
    );
    if (result !== undefined && !result.ok) setError(result.error.message);
  };

  const close = (): void => {
    void modelPopupApi.close().then((result) => {
      if (!result.ok) setError(result.error.message);
    });
  };

  return <main className="model-popup-shell">
    <section className="model-popup-card" aria-label="モデル変更">
      <header className="model-popup-header">
        <div className="model-popup-title">
          <AvatarBlob color={snapshot?.config.ui.avatarColor} image={snapshot?.avatarImagePng} size={24} />
          <div><span>モデル変更</span><small>{snapshot?.companionDisplayName ?? "Coo"}</small></div>
        </div>
        <button className="model-popup-close" type="button" aria-label="モデル変更を閉じる" onClick={close}><CloseIcon /></button>
      </header>
      {snapshot === undefined
        ? <p className="model-popup-status" role={error === undefined ? undefined : "alert"}>{error ?? "読み込んでいます…"}</p>
        : <CompanionModelControls
          snapshot={snapshot}
          catalog={catalog}
          disabled={modelPopupControlsDisabled(snapshot)}
          onSave={save}
          onReloadOpencode={reloadOpencode}
        />}
      {snapshot !== undefined && error !== undefined ? <p className="model-popup-error" role="alert">{error}</p> : null}
    </section>
  </main>;
}
