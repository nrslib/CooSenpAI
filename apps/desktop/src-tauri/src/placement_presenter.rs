use crate::ui_events::{UiEffect, UiEvent, UiTask};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MainWindowPlacement {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug)]
pub(crate) enum PlacementEvent {
    Changed {
        path: PathBuf,
        placement: MainWindowPlacement,
    },
    SaveExpired(u64),
    Saved(Result<(), String>),
}

#[derive(Default)]
pub(crate) struct PlacementPresenter {
    revision: u64,
    pending: Option<(PathBuf, MainWindowPlacement)>,
    ready: bool,
    saving: bool,
}

impl PlacementPresenter {
    pub(crate) fn handle(&mut self, event: PlacementEvent) -> Vec<UiEffect> {
        let mut effects = Vec::new();
        match event {
            PlacementEvent::Changed { path, placement } => {
                self.revision = self.revision.saturating_add(1);
                self.pending = Some((path, placement));
                self.ready = false;
                effects.push(UiEffect::Spawn(UiTask::Delay {
                    duration: std::time::Duration::from_millis(250),
                    event: UiEvent::Placement(PlacementEvent::SaveExpired(self.revision)),
                }));
            }
            PlacementEvent::SaveExpired(revision) if revision == self.revision => {
                self.ready = true;
            }
            PlacementEvent::Saved(result) => {
                self.saving = false;
                if let Err(error) = result {
                    effects.push(UiEffect::Log(format!(
                        "ウィンドウ位置の保存に失敗しました: {error}"
                    )));
                }
            }
            PlacementEvent::SaveExpired(_) => {}
        }
        if self.ready && !self.saving {
            if let Some((path, placement)) = self.pending.take() {
                self.saving = true;
                self.ready = false;
                effects.push(UiEffect::Spawn(UiTask::SavePlacement { path, placement }));
            }
        }
        effects
    }
}

pub(crate) async fn save(path: PathBuf, placement: MainWindowPlacement) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let bytes = serde_json::to_vec_pretty(&placement).map_err(|error| error.to_string())?;
        coosenpai_core::persistence::atomic_write_bytes(&path, &bytes)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

