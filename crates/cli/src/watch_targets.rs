use super::WatchTargetAction;
use anyhow::Result;
use coosenpai_core::config::{load_config, patch_config, ConfigPaths, WatchAppConfig};

pub(super) fn run(paths: &ConfigPaths, action: WatchTargetAction) -> Result<()> {
    match action {
        WatchTargetAction::List => print(&load_config(paths)?.watch.apps),
        WatchTargetAction::Add { application } => {
            let running = crate::platform::MacApplicationCapture::running()?;
            let selected = running.into_iter().find(|candidate| {
                candidate.bundle_id == application
                    || candidate.name.eq_ignore_ascii_case(application.trim())
            });
            let Some(selected) = selected else {
                anyhow::bail!("起動中のアプリが見つかりません: {application}")
            };
            let config = patch_config(paths, None, move |mut config| {
                if let Some(existing) = config
                    .watch
                    .apps
                    .iter_mut()
                    .find(|candidate| candidate.bundle_id == selected.bundle_id)
                {
                    existing.name = selected.name;
                    existing.enabled = true;
                } else {
                    config.watch.apps.push(WatchAppConfig {
                        bundle_id: selected.bundle_id,
                        name: selected.name,
                        enabled: true,
                    });
                }
                Ok(config)
            })?;
            print(&config.watch.apps)
        }
        WatchTargetAction::Remove { application } => {
            let config = patch_config(paths, None, move |mut config| {
                let before = config.watch.apps.len();
                config.watch.apps.retain(|candidate| {
                    candidate.bundle_id != application
                        && !candidate.name.eq_ignore_ascii_case(application.trim())
                });
                if config.watch.apps.len() == before {
                    return Err(coosenpai_core::config::ConfigError::Validation(vec![
                        coosenpai_core::config::ConfigValidationIssue {
                            path: "watch.apps".to_owned(),
                            message: format!("見守り対象が見つかりません: {application}"),
                        },
                    ]));
                }
                Ok(config)
            })?;
            print(&config.watch.apps)
        }
    }
}

fn print(value: &[WatchAppConfig]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
