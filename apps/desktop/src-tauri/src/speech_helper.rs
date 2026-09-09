use coosenpai_core::ports::HelperResolverPort;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(super) fn resolve(
    executable_dir: Option<&Path>,
    root: &Path,
) -> Result<Option<PathBuf>, String> {
    let managed = std::env::var_os("COOSENPAI_E2E_MANAGED").is_some_and(|value| value == "1");
    if cfg!(feature = "e2e-fixtures") && managed {
        if let Some(fixture) = std::env::var_os("COOSENPAI_E2E_SPEECH_HELPER") {
            let artifacts = std::env::var_os("COOSENPAI_E2E_ARTIFACTS")
                .ok_or("音声 E2E helper には隔離 artifact root が必要です")?;
            return validate_fixture(Path::new(&fixture), Path::new(&artifacts), root).map(Some);
        }
    }
    Ok(executable_dir.and_then(|directory| {
        crate::platform::MacHelperResolver.resolve_speech_helper(directory, root)
    }))
}

fn validate_fixture(fixture: &Path, artifacts: &Path, root: &Path) -> Result<PathBuf, String> {
    if !fixture.is_absolute() || !artifacts.is_absolute() {
        return Err("音声 E2E helper と artifact root は絶対パスが必要です".to_owned());
    }
    let artifacts = artifacts
        .canonicalize()
        .map_err(|error| format!("音声 E2E artifact root: {error}"))?;
    let metadata = artifacts
        .metadata()
        .map_err(|error| format!("音声 E2E artifact root: {error}"))?;
    if artifacts.parent().is_none()
        || !metadata.is_dir()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("音声 E2E artifact root は専用の非公開ディレクトリが必要です".to_owned());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("音声 E2E config root: {error}"))?;
    if root != artifacts.join("home/.coosenpai") {
        return Err("音声 E2E helper は artifact 内の隔離 HOME でのみ使用できます".to_owned());
    }
    let fixture = fixture
        .canonicalize()
        .map_err(|error| format!("音声 E2E helper: {error}"))?;
    let metadata = fixture
        .metadata()
        .map_err(|error| format!("音声 E2E helper: {error}"))?;
    if !fixture.starts_with(&artifacts)
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("音声 E2E helper は artifact 内の実行可能ファイルが必要です".to_owned());
    }
    Ok(fixture)
}

