use anyhow::{Context, Result};
use coosenpai_core::config::{ensure_layout, load_config, Config, ConfigPaths};
use std::path::PathBuf;

pub(super) fn load() -> Result<(PathBuf, ConfigPaths, Config)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME が設定されていません")?;
    let paths = ConfigPaths::for_home(&home)
        .with_builtin_personas(super::repository_root().join("builtins/prompts/facets/personas"))
        .with_builtin_tutorial(super::repository_root().join("builtins/tutorial/tutorial.md"));
    ensure_layout(&paths)?;
    let config = load_config(&paths)?;
    Ok((home, paths, config))
}
