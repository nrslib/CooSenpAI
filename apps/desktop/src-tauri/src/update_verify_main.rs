mod update_archive;
mod update_format;

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("更新ファイルが上限を超えています".into());
    }
    Ok(bytes)
}

fn verify() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("使い方: coosenpai-update-verify ARCHIVE VERSION".into());
    }
    let path = Path::new(&args[0]);
    let version: semver::Version = args[1]
        .to_str()
        .ok_or("版が UTF-8 ではありません")?
        .parse()?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("ファイル名が不正です")?;
    let architecture = ["aarch64", "x86_64"]
        .into_iter()
        .find(|architecture| filename == format!("CooSenpAI_{version}_{architecture}.app.tar.gz"))
        .ok_or("更新ファイル名の版または architecture が不正です")?;
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))?;
    let pubkey = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .ok_or("公開鍵がありません")?;
    let mut signature_path = path.as_os_str().to_os_string();
    signature_path.push(".sig");
    let signature = String::from_utf8(read_bounded(Path::new(&signature_path), 16 * 1024)?)?;
    let archive = update_format::verify(
        read_bounded(path, update_format::MAX_ARCHIVE_BYTES)?,
        &update_format::signature(&signature)?,
        &update_format::public_key(pubkey)?,
    )?;
    let temporary = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let staged = update_archive::prepare(&archive, temporary.path(), &version, None, architecture)?;
    staged.close()?;
    println!("更新署名・アーカイブ構造・アプリ識別子・版の検証が完了しました");
    Ok(())
}

fn main() {
    if let Err(error) = verify() {
        eprintln!("更新ファイルの検証に失敗しました: {error}");
        std::process::exit(1);
    }
}
