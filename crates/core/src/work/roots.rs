use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AllowedRoot {
    pub path: PathBuf,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkKind {
    Investigate,
    Work,
}

impl WorkKind {
    pub fn requires_write(self) -> bool {
        matches!(self, WorkKind::Work)
    }

    pub fn label(self) -> &'static str {
        match self {
            WorkKind::Investigate => "調査（読み取りのみ）",
            WorkKind::Work => "作業（編集・実行あり）",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum RootStatus {
    Inside { root: PathBuf },
    Outside,
    Excluded { root: PathBuf },
}

impl RootStatus {
    pub fn is_inside(&self) -> bool {
        matches!(self, RootStatus::Inside { .. })
    }
}

/// 認証情報の置き場。ホーム相対で固定し、承認でも解除しない。
pub const EXCLUDED_ROOT_SUFFIXES: [&str; 5] =
    [".ssh", ".aws", ".gnupg", "Library/Keychains", ".config/gh"];

#[derive(Debug, Clone)]
pub struct RootPolicy {
    home: PathBuf,
    roots: Vec<AllowedRoot>,
}

impl RootPolicy {
    pub fn new(home: PathBuf, roots: Vec<AllowedRoot>) -> Self {
        Self {
            home: real_path(&home),
            roots,
        }
    }

    pub fn roots(&self) -> &[AllowedRoot] {
        &self.roots
    }

    /// 除外ルートも実体パスで持つ。シンボリックリンク経由で canonical 済みの cwd と
    /// 食い違い、認証情報の置き場への判定が素通しになるのを防ぐ。
    pub fn excluded_roots(&self) -> Vec<PathBuf> {
        EXCLUDED_ROOT_SUFFIXES
            .iter()
            .map(|suffix| real_path(&self.home.join(suffix)))
            .collect()
    }

    /// `~` と `~/...` をホームへ展開する。それ以外はそのまま返す。
    pub fn expand_home(&self, path: &str) -> PathBuf {
        match path.strip_prefix('~') {
            Some("") => self.home.clone(),
            Some(rest) if rest.starts_with('/') => self.home.join(&rest[1..]),
            _ => PathBuf::from(path),
        }
    }

    /// `cwd` は正規化済みの絶対パス。除外ルートは内側でも外側でも拒否する。
    pub fn classify(&self, cwd: &Path, kind: WorkKind) -> RootStatus {
        if let Some(root) = self
            .excluded_roots()
            .into_iter()
            .find(|root| cwd.starts_with(root) || root.starts_with(cwd))
        {
            return RootStatus::Excluded { root };
        }
        self.roots
            .iter()
            .filter(|root| root.read && (!kind.requires_write() || root.write))
            .map(|root| real_path(&root.path))
            .filter(|root| cwd.starts_with(root))
            .max_by_key(|root| root.as_os_str().len())
            .map_or(RootStatus::Outside, |root| RootStatus::Inside { root })
    }
}

/// 設定のルートは利用者が書いた表記で保存する。存在するルートは実体パスで比較し、
/// 正規化済みの cwd（/private/tmp など）と食い違わないようにする。
fn real_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

