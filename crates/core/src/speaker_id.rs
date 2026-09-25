//! 内部話者 ID の共通契約。
//!
//! 話者 ID は登録時に発行した UUID をそのまま永続化し、短縮表示は各 UI 層だけで行う。

use uuid::Uuid;

/// 内部話者 ID が、生成時と同じ小文字のハイフン区切り UUID かを判定する。
pub fn is_valid_speaker_id(value: &str) -> bool {
    Uuid::parse_str(value)
        .ok()
        .is_some_and(|uuid| uuid.hyphenated().to_string() == value)
}
