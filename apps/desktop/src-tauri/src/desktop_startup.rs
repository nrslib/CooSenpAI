use crate::state::DesktopState;
use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;
use tauri::Manager;

pub(super) fn prepare_paths(
    home: Option<std::ffi::OsString>,
    application_home: Option<std::ffi::OsString>,
) -> anyhow::Result<coosenpai_core::config::ConfigPaths> {
    use anyhow::Context;
    use coosenpai_core::config::{ensure_layout, ConfigPaths};
    use std::path::PathBuf;
    let home = PathBuf::from(home.context("HOME を取得できません")?);
    anyhow::ensure!(
        home.is_absolute() && home.is_dir(),
        "HOME が存在するディレクトリではありません: {}",
        home.display()
    );
    let paths = match application_home {
        Some(root) => {
            let root = PathBuf::from(root);
            anyhow::ensure!(
                root.is_absolute(),
                "COOSENPAI_HOME は絶対パスで指定してください: {}",
                root.display()
            );
            ConfigPaths::from_root(root)
        }
        None => ConfigPaths::for_home(&home),
    };
    ensure_layout(&paths)
        .with_context(|| format!("設定ディレクトリを準備できません: {}", paths.root.display()))?;
    Ok(paths)
}

pub(super) trait SetupFailurePort {
    fn report(&self, message: &str);
    fn exit(&self);
}

impl SetupFailurePort for tauri::App {
    fn report(&self, message: &str) {
        if let Some(state) = self.try_state::<Arc<DesktopState>>() {
            if let Err(error) = state.logger.write("ERROR", message) {
                eprintln!("起動エラーのログ記録に失敗しました: {error}");
            }
        }
        eprintln!("{message}");
    }

    fn exit(&self) {
        self.handle().exit(1);
    }
}

pub(super) fn complete(
    result: anyhow::Result<()>,
    port: &impl SetupFailurePort,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(error) = result {
        port.report(&format!(
            "CooSenpAI desktop の起動に失敗しました: {error:#}"
        ));
        port.exit();
    }
    // Tauri は setup の Err を Cocoa コールバック内で panic にするため、終了要求後も Ok を返す。
    Ok(())
}

