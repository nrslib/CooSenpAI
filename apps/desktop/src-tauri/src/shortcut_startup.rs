use crate::capture::{self, ShortcutBindings};
use crate::state::DesktopState;
use coosenpai_core::ports::RuntimeLogger;

pub(crate) async fn sync(state: &DesktopState, bindings: ShortcutBindings, config_version: u64) {
    if let Err(error) = capture::sync_shortcuts(state, bindings, config_version).await {
        let _ = state.logger.write(
            "WARN",
            &format!("グローバルショートカットの起動時同期に失敗しました: {error}"),
        );
    }
}
