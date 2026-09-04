use crate::config::ConfigPaths;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaProfile {
    pub id: String,
    pub body: String,
}

#[derive(Debug, Error)]
pub enum PersonaError {
    #[error("persona を読み込めません: {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("組み込み persona の場所がありません")]
    MissingBuiltinDirectory,
}

pub fn load_persona(paths: &ConfigPaths, id: &str) -> Result<PersonaProfile, PersonaError> {
    let builtin = paths
        .builtin_personas
        .as_ref()
        .ok_or(PersonaError::MissingBuiltinDirectory)?
        .join(format!("{id}.md"));
    let user = paths.personas.join(format!("{id}.md"));
    let path = if builtin.is_file() { builtin } else { user };
    let document = fs::read_to_string(&path).map_err(|source| PersonaError::Io {
        path: path.clone(),
        source,
    })?;
    parse_persona(id, &document)
}

pub fn parse_persona(id: &str, document: &str) -> Result<PersonaProfile, PersonaError> {
    Ok(PersonaProfile {
        id: id.to_owned(),
        body: document.to_owned(),
    })
}

