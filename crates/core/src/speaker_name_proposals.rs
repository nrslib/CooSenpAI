//! 話者名登録の提案と、確認後の操作に必要な根拠を保存する。

use crate::config::ConfigPaths;
use crate::persistence::{atomic_write_json, JsonlStore, PersistenceError, SiblingLock};
use crate::speaker_id::PromptSpeakerIdResolver;
use crate::speaker_names::validate_speaker_name;
use crate::state::{SpeakerIdentificationStatus, TranscriptRecord};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use uuid::Uuid;

const SCHEMA_VERSION: u8 = 5;
const MAX_PROPOSALS: usize = 200;
const MAX_PENDING_CONFLICTS: usize = 20;
const MAX_CONFLICT_OPTIONS: usize = 5;
const MAX_EVIDENCE: usize = 3;
const MAX_QUOTE_CHARS: usize = 160;
const MAX_GROUP_ID_CHARS: usize = 128;
const MAX_ADDRESS_RESPONSE_GAP_MS: u64 = 5_000;
pub const INFERENCE_REJECTION_TTL: Duration = Duration::days(30);
const PROPOSAL_TTL: Duration = Duration::days(7);
const PENDING_CONFLICT_TTL: Duration = Duration::hours(1);

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerNameProposalSource {
    #[default]
    UserRequest,
    Inferred,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerNameEvidenceRole {
    SelfIntroduction,
    Address,
    Response,
    ThirdPartyMention,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeakerNameInferenceTerms {
    patterns: Vec<String>,
    request_tails: Vec<String>,
    negative_question_markers: Vec<String>,
    reconsideration_markers: Vec<String>,
}

fn speaker_name_inference_terms() -> &'static SpeakerNameInferenceTerms {
    static TERMS: OnceLock<SpeakerNameInferenceTerms> = OnceLock::new();
    TERMS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../fixtures/prompts/speaker-name-inference-phrases.json"
        ))
        .expect("speaker name inference phrases are valid")
    })
}

pub fn user_message_requests_speaker_name_inference(message: &str) -> bool {
    inference_request_match(message).is_some()
}

fn inference_request_match(message: &str) -> Option<(usize, usize)> {
    let terms = speaker_name_inference_terms();
    terms.patterns.iter().find_map(|pattern| {
        message.match_indices(pattern).find_map(|(start, matched)| {
            if is_inside_quote(message, start) {
                return None;
            }
            let end = start + matched.len();
            let suffix = trim_ecmascript_whitespace_end(&message[end..]);
            let clause_end = suffix
                .char_indices()
                .find(|(_, character)| {
                    matches!(character, '。' | '!' | '！' | '?' | '？' | '\r' | '\n')
                })
                .map(|(index, character)| index + character.len_utf8())
                .unwrap_or(suffix.len());
            let clause = trim_ecmascript_whitespace_end(&suffix[..clause_end]);
            let normalized_clause = clause.to_lowercase();
            if terms
                .negative_question_markers
                .iter()
                .any(|marker| normalized_clause.contains(&marker.to_lowercase()))
            {
                return None;
            }
            let question_terminated = clause.ends_with('?') || clause.ends_with('？');
            let tail = clause.trim_end_matches(['。', '!', '！', '?', '？', '\r', '\n']);
            let tail = trim_ecmascript_whitespace_end(tail);
            let requested = question_terminated
                || terms
                    .request_tails
                    .iter()
                    .any(|request_tail| tail == request_tail);
            requested.then_some((start, end))
        })
    })
}

fn user_message_reconsiders_speaker_name_inference(message: &str) -> bool {
    let Some((start, _)) = inference_request_match(message) else {
        return false;
    };
    speaker_name_inference_terms()
        .reconsideration_markers
        .iter()
        .any(|marker| message[..start].contains(marker))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpeakerNameRequestTerms {
    subject_markers: Vec<String>,
    actions: Vec<String>,
    request_tail_markers: Vec<String>,
    sentence_endings: Vec<String>,
    quote_pairs: Vec<(String, String)>,
}

fn speaker_name_request_terms() -> &'static SpeakerNameRequestTerms {
    static TERMS: OnceLock<SpeakerNameRequestTerms> = OnceLock::new();
    TERMS.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../fixtures/prompts/speaker-name-request-phrases.json"
        ))
        .expect("speaker name request phrases are valid")
    })
}

pub fn user_message_requests_speaker_name(message: &str) -> bool {
    has_direct_speaker_name_action(message)
}

fn has_direct_speaker_name_action(message: &str) -> bool {
    let terms = speaker_name_request_terms();
    terms.actions.iter().any(|action| {
        message
            .match_indices(action)
            .any(|(action_index, matched)| {
                let action_end = action_index + matched.len();
                if is_inside_quote(message, action_index)
                    || !has_unquoted_subject_marker(&message[..action_index])
                {
                    return false;
                }
                has_direct_request_tail(&message[action_end..])
            })
    })
}

fn has_unquoted_subject_marker(message: &str) -> bool {
    speaker_name_request_terms()
        .subject_markers
        .iter()
        .any(|marker| {
            message
                .match_indices(marker)
                .any(|(index, _)| !is_inside_quote(message, index))
        })
}

fn has_direct_request_tail(tail: &str) -> bool {
    let terms = speaker_name_request_terms();
    let mut remainder = trim_ecmascript_whitespace_end(tail);
    while let Some(ending) = terms
        .sentence_endings
        .iter()
        .find(|ending| remainder.ends_with(ending.as_str()))
    {
        remainder = trim_ecmascript_whitespace_end(&remainder[..remainder.len() - ending.len()]);
    }
    remainder.is_empty()
        || terms
            .request_tail_markers
            .iter()
            .any(|marker| remainder == marker)
}

fn trim_ecmascript_whitespace_end(value: &str) -> &str {
    value.trim_end_matches(is_ecmascript_whitespace)
}

fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn is_inside_quote(message: &str, byte_index: usize) -> bool {
    let mut closers = Vec::new();
    for (index, character) in message.char_indices() {
        if index >= byte_index {
            break;
        }
        if let Some(expected) = closers.last().copied() {
            if character == expected {
                closers.pop();
            }
        } else if let Some((_, closing)) = speaker_name_request_terms()
            .quote_pairs
            .iter()
            .find(|(opening, _)| opening.starts_with(character))
        {
            closers.push(closing.chars().next().expect("quote closer"));
        }
    }
    !closers.is_empty()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameProposalEvidence {
    pub observation_id: String,
    pub audio_start_ms: u64,
    pub audio_end_ms: u64,
    pub quote: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<SpeakerNameEvidenceRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

/// Provider が返す未信頼の候補。registryId は含めず、根拠から Rust が復元する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameProposalCandidate {
    pub speaker_id: String,
    pub name: String,
    pub source_user_message_ids: Vec<String>,
    pub evidence: Vec<SpeakerNameProposalEvidence>,
}

#[derive(Clone, Copy)]
pub struct SpeakerNameProposalAcceptanceContext<'a> {
    pub user_messages: &'a [(String, String)],
    pub audio_log_index: Option<&'a Value>,
    pub current_observation: Option<&'a Value>,
    pub resolver: &'a PromptSpeakerIdResolver,
    pub active_registry_id: &'a str,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerNameProposalStatus {
    Proposed,
    Rejected,
    Applying,
    Applied,
    Undoing,
    Undone,
    Conflict,
    Expired,
}

impl SpeakerNameProposalStatus {
    pub fn has_unresolved_operation(self) -> bool {
        matches!(self, Self::Applying | Self::Undoing)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerNameProposalResultRecordStatus {
    Pending,
    Recorded,
    Pruned,
    LegacyRecorded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameProposalResultRecord {
    pub status: SpeakerNameProposalResultRecordStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerNameProposalUndoCardStatus {
    #[default]
    LegacyUnknown,
    Pending,
    Delivering,
    Delivered,
    Dismissed,
    Expired,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameApplyOperation {
    pub id: String,
    pub before_name: Option<String>,
    pub after_name: String,
    pub undo_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredSpeakerNameProposal {
    pub id: String,
    pub version: u64,
    pub registry_id: String,
    pub speaker_id: String,
    pub current_name: Option<String>,
    pub name: String,
    pub name_edited_by_user: bool,
    #[serde(default)]
    pub source: SpeakerNameProposalSource,
    #[serde(default)]
    pub inferred_candidate_name: Option<String>,
    pub source_user_message_ids: Vec<String>,
    #[serde(default)]
    pub request_fingerprint: String,
    pub evidence: Vec<SpeakerNameProposalEvidence>,
    pub evidence_transcript_paths: Vec<String>,
    pub authorized_transcript_paths: Vec<String>,
    pub status: SpeakerNameProposalStatus,
    pub created_at: String,
    pub expires_at: String,
    pub confirmation_bubble_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_bubble_dismissal_id: Option<String>,
    #[serde(default)]
    pub suppressed_until: Option<String>,
    pub operation: Option<SpeakerNameApplyOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_record: Option<SpeakerNameProposalResultRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_completion_record: Option<SpeakerNameProposalResultRecord>,
    #[serde(default)]
    pub undo_card_status: SpeakerNameProposalUndoCardStatus,
}

impl StoredSpeakerNameProposal {
    pub fn can_undo_at(&self, now: &DateTime<Utc>) -> bool {
        self.status == SpeakerNameProposalStatus::Applied
            && self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.undo_id.is_none())
            && DateTime::parse_from_rfc3339(&self.expires_at)
                .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > *now)
    }

    pub fn should_deliver_undo_card_at(&self, now: &DateTime<Utc>) -> bool {
        self.can_undo_at(now)
            && matches!(
                self.undo_card_status,
                SpeakerNameProposalUndoCardStatus::Pending
                    | SpeakerNameProposalUndoCardStatus::Delivering
            )
    }
}

fn valid_result_record(record: Option<&SpeakerNameProposalResultRecord>) -> bool {
    let valid_timestamp = |value: Option<&String>| {
        value.is_none_or(|value| DateTime::parse_from_rfc3339(value).is_ok())
    };
    match record {
        None => true,
        Some(record) => match record.status {
            SpeakerNameProposalResultRecordStatus::Pending => {
                record.recorded_at.is_none() && valid_timestamp(record.entry_created_at.as_ref())
            }
            SpeakerNameProposalResultRecordStatus::Recorded => {
                record.conversation_generation.is_some()
                    && record.entry_created_at.is_some()
                    && record.recorded_at.is_some()
                    && valid_timestamp(record.entry_created_at.as_ref())
                    && valid_timestamp(record.recorded_at.as_ref())
            }
            SpeakerNameProposalResultRecordStatus::Pruned => {
                record.entry_created_at.is_some()
                    && valid_timestamp(record.entry_created_at.as_ref())
                    && valid_timestamp(record.recorded_at.as_ref())
            }
            SpeakerNameProposalResultRecordStatus::LegacyRecorded => true,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSpeakerNameConflictOption {
    pub name: String,
    pub source_user_message_ids: Vec<String>,
    pub evidence: Vec<SpeakerNameProposalEvidence>,
    pub evidence_transcript_paths: Vec<String>,
    pub authorized_transcript_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingSpeakerNameConflict {
    pub id: String,
    pub registry_id: String,
    pub speaker_id: String,
    pub options: Vec<PendingSpeakerNameConflictOption>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameProposalConflict {
    pub options: Vec<PendingSpeakerNameConflictOption>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeakerNameProposalAcceptance {
    pub proposals: Vec<StoredSpeakerNameProposal>,
    pub conflicts: Vec<SpeakerNameProposalConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerNameProposalStore {
    pub schema_version: u8,
    pub proposals: Vec<StoredSpeakerNameProposal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_conflicts: Vec<PendingSpeakerNameConflict>,
}

impl Default for SpeakerNameProposalStore {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            proposals: Vec::new(),
            pending_conflicts: Vec::new(),
        }
    }
}

pub fn load_speaker_name_proposals(
    paths: &ConfigPaths,
) -> Result<SpeakerNameProposalStore, PersistenceError> {
    load_speaker_name_proposals_at(paths, Utc::now())
}

pub(crate) fn pending_speaker_name_conflict_selection_may_be_in_progress(
    paths: &ConfigPaths,
    message: &str,
    active_registry_id: &str,
) -> Result<bool, PersistenceError> {
    if Uuid::parse_str(active_registry_id).is_err() {
        return Ok(false);
    }
    let store = load_speaker_name_proposals(paths)?;
    Ok(store.pending_conflicts.iter().any(|conflict| {
        conflict.registry_id == active_registry_id
            && (selected_conflict_option(message, &conflict.options).is_some()
                || conflict_selection_attempt_mentions_option(message, &conflict.options))
    }))
}

fn load_speaker_name_proposals_at(
    paths: &ConfigPaths,
    now: DateTime<Utc>,
) -> Result<SpeakerNameProposalStore, PersistenceError> {
    let lock_path = paths.speaker_name_proposals.with_extension("json.lock");
    let _lock = SiblingLock::acquire(&lock_path)?;
    let Some(mut store) = read_speaker_name_proposals(paths)? else {
        return Ok(SpeakerNameProposalStore::default());
    };
    let original = store.clone();
    normalize_speaker_name_proposal_store(&mut store)?;
    prune_expired_pending_conflicts(&mut store, now);
    validate_store(&store)?;
    if store != original {
        atomic_write_json(&paths.speaker_name_proposals, &store)?;
    }
    Ok(store)
}

fn read_speaker_name_proposals(
    paths: &ConfigPaths,
) -> Result<Option<SpeakerNameProposalStore>, PersistenceError> {
    let bytes = match fs::read(&paths.speaker_name_proposals) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(PersistenceError::Io(error)),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub fn update_speaker_name_proposals<T>(
    paths: &ConfigPaths,
    update: impl FnOnce(&mut SpeakerNameProposalStore) -> Result<T, PersistenceError>,
) -> Result<T, PersistenceError> {
    let lock_path = paths.speaker_name_proposals.with_extension("json.lock");
    let _lock = SiblingLock::acquire(&lock_path)?;
    let mut store = read_speaker_name_proposals(paths)?.unwrap_or_default();
    normalize_speaker_name_proposal_store(&mut store)?;
    let result = update(&mut store)?;
    store.schema_version = SCHEMA_VERSION;
    normalize_speaker_name_proposal_store(&mut store)?;
    validate_store(&store)?;
    atomic_write_json(&paths.speaker_name_proposals, &store)?;
    Ok(result)
}

fn normalize_speaker_name_proposal_store(
    store: &mut SpeakerNameProposalStore,
) -> Result<(), PersistenceError> {
    if !matches!(store.schema_version, 1 | 2 | 3 | 4 | SCHEMA_VERSION) {
        return Err(PersistenceError::Invalid(
            "話者名提案ストアの schemaVersion が不正です".to_owned(),
        ));
    }
    // schemaVersion 1 に source がなければ、従来の明示依頼として扱う。
    store.schema_version = SCHEMA_VERSION;
    for proposal in &mut store.proposals {
        if proposal.request_fingerprint.is_empty() && !proposal.evidence.is_empty() {
            proposal.request_fingerprint = proposal_fingerprint(
                &proposal.registry_id,
                &proposal.speaker_id,
                &proposal.name,
                &proposal.source_user_message_ids,
                &proposal.evidence,
            )?;
        }
        if is_terminal_status(proposal.status) {
            redact_terminal_evidence(proposal);
        }
    }
    Ok(())
}

fn is_terminal_status(status: SpeakerNameProposalStatus) -> bool {
    matches!(
        status,
        SpeakerNameProposalStatus::Rejected
            | SpeakerNameProposalStatus::Applied
            | SpeakerNameProposalStatus::Undone
            | SpeakerNameProposalStatus::Conflict
            | SpeakerNameProposalStatus::Expired
    )
}

fn redact_terminal_evidence(proposal: &mut StoredSpeakerNameProposal) {
    proposal.source_user_message_ids.clear();
    proposal.evidence.clear();
    proposal.evidence_transcript_paths.clear();
    proposal.authorized_transcript_paths.clear();
}

fn proposal_fingerprint(
    registry_id: &str,
    speaker_id: &str,
    name: &str,
    source_user_message_ids: &[String],
    evidence: &[SpeakerNameProposalEvidence],
) -> Result<String, PersistenceError> {
    let source_ids = normalized_source_message_ids(source_user_message_ids);
    let evidence = normalized_proposal_evidence(evidence);
    let serialized = serde_json::to_vec(&(registry_id, speaker_id, name, source_ids, evidence))?;
    Ok(format!("{:x}", Sha256::digest(serialized)))
}

fn normalized_source_message_ids(source_user_message_ids: &[String]) -> Vec<String> {
    let mut source_ids = source_user_message_ids.to_vec();
    source_ids.sort();
    source_ids
}

fn normalized_proposal_evidence(
    evidence: &[SpeakerNameProposalEvidence],
) -> Vec<SpeakerNameProposalEvidence> {
    let mut normalized = evidence.to_vec();
    normalized.sort_by(|left, right| {
        (
            &left.observation_id,
            left.audio_start_ms,
            left.audio_end_ms,
            left.role,
            &left.group_id,
            &left.quote,
        )
            .cmp(&(
                &right.observation_id,
                right.audio_start_ms,
                right.audio_end_ms,
                right.role,
                &right.group_id,
                &right.quote,
            ))
    });
    normalized
}

pub(crate) fn same_proposal_candidate(
    left: &SpeakerNameProposalCandidate,
    right: &SpeakerNameProposalCandidate,
) -> bool {
    left.speaker_id == right.speaker_id
        && left.name == right.name
        && normalized_source_message_ids(&left.source_user_message_ids)
            == normalized_source_message_ids(&right.source_user_message_ids)
        && normalized_proposal_evidence(&left.evidence)
            == normalized_proposal_evidence(&right.evidence)
}

pub(crate) fn candidate_matches_stored_proposal(
    candidate: &SpeakerNameProposalCandidate,
    proposal: &StoredSpeakerNameProposal,
) -> bool {
    candidate.speaker_id == proposal.speaker_id
        && candidate.name == proposal.name
        && normalized_source_message_ids(&candidate.source_user_message_ids)
            == normalized_source_message_ids(&proposal.source_user_message_ids)
        && normalized_proposal_evidence(&candidate.evidence)
            == normalized_proposal_evidence(&proposal.evidence)
}

fn ensure_proposal_capacity(store: &mut SpeakerNameProposalStore) -> Result<(), PersistenceError> {
    if store.proposals.len() < MAX_PROPOSALS {
        return Ok(());
    }
    if let Some(index) = store.proposals.iter().rposition(|proposal| {
        is_terminal_status(proposal.status)
            && proposal.confirmation_bubble_id.is_none()
            && proposal.pending_bubble_dismissal_id.is_none()
    }) {
        store.proposals.remove(index);
        return Ok(());
    }
    Err(PersistenceError::Invalid(
        "話者名提案ストアの空き容量がありません".to_owned(),
    ))
}

/// Provider 候補を、現在のユーザー入力・許可済み transcript・識別済み期間で検証して保存する。
pub fn accept_speaker_name_proposals(
    paths: &ConfigPaths,
    candidates: &[SpeakerNameProposalCandidate],
    context: SpeakerNameProposalAcceptanceContext<'_>,
    now: DateTime<Utc>,
) -> Result<Vec<StoredSpeakerNameProposal>, PersistenceError> {
    Ok(accept_speaker_name_proposals_with_conflicts(paths, candidates, context, now)?.proposals)
}

pub fn accept_speaker_name_proposals_with_conflicts(
    paths: &ConfigPaths,
    candidates: &[SpeakerNameProposalCandidate],
    context: SpeakerNameProposalAcceptanceContext<'_>,
    now: DateTime<Utc>,
) -> Result<SpeakerNameProposalAcceptance, PersistenceError> {
    let SpeakerNameProposalAcceptanceContext {
        user_messages,
        audio_log_index,
        current_observation,
        resolver,
        active_registry_id,
    } = context;
    if candidates.is_empty() || Uuid::parse_str(active_registry_id).is_err() {
        return Ok(SpeakerNameProposalAcceptance::default());
    }
    let user_ids = user_messages
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<BTreeSet<_>>();
    let user_by_id = user_messages
        .iter()
        .map(|(id, message)| (id.as_str(), message.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let allowed_paths = indexed_transcript_paths(paths, audio_log_index, current_observation)?;
    let allowed_ref_paths =
        indexed_transcript_ref_paths(paths, audio_log_index, current_observation)?;
    if allowed_paths.is_empty() || allowed_paths.len() > 3 {
        return Ok(SpeakerNameProposalAcceptance::default());
    }
    let mut records = Vec::new();
    let mut records_by_path = std::collections::BTreeMap::new();
    for path in allowed_paths.keys() {
        let path_records = JsonlStore::new(path.clone()).read::<TranscriptRecord>()?;
        records.extend(path_records.iter().cloned());
        records_by_path.insert(path.clone(), path_records);
    }

    let mut verified_candidates = Vec::new();
    for candidate in candidates.iter().take(5) {
        let Some(verified) = verify_candidate(
            paths,
            candidate,
            &user_ids,
            &user_by_id,
            &records,
            &records_by_path,
            &allowed_ref_paths,
            &allowed_paths,
            resolver,
            active_registry_id,
        )?
        else {
            continue;
        };
        verified_candidates.push(verified);
    }
    verified_candidates.sort_by(|left, right| {
        let source_priority = |source| match source {
            SpeakerNameProposalSource::UserRequest => 0,
            SpeakerNameProposalSource::Inferred => 1,
        };
        (
            left.registry_id.as_str(),
            left.speaker_id.as_str(),
            left.name.as_str(),
            source_priority(left.source),
        )
            .cmp(&(
                right.registry_id.as_str(),
                right.speaker_id.as_str(),
                right.name.as_str(),
                source_priority(right.source),
            ))
    });

    let registered_names = crate::speaker_names::load_speaker_names(paths, active_registry_id)?;
    let registered_speaker_ids = registered_names.keys().cloned().collect::<BTreeSet<_>>();
    verified_candidates.retain(|candidate| {
        candidate.source != SpeakerNameProposalSource::Inferred
            || !registered_speaker_ids.contains(&candidate.speaker_id)
    });
    if !registered_speaker_ids.is_empty() {
        update_speaker_name_proposals(paths, |store| {
            store.pending_conflicts.retain(|conflict| {
                conflict.registry_id != active_registry_id
                    || !registered_speaker_ids.contains(&conflict.speaker_id)
            });
            Ok(())
        })?;
    }

    let mut names_by_speaker = BTreeMap::<(String, String), BTreeSet<String>>::new();
    for verified in &verified_candidates {
        if verified.source != SpeakerNameProposalSource::Inferred {
            continue;
        }
        names_by_speaker
            .entry((verified.registry_id.clone(), verified.speaker_id.clone()))
            .or_default()
            .insert(verified.name.clone());
    }
    let mut conflicting_speakers = names_by_speaker
        .into_iter()
        .filter_map(|(speaker, names)| (names.len() > 1).then_some(speaker))
        .collect::<BTreeSet<_>>();
    let explicit_requests = verified_candidates
        .iter()
        .filter(|candidate| candidate.source == SpeakerNameProposalSource::UserRequest)
        .map(|candidate| (candidate.registry_id.clone(), candidate.speaker_id.clone()))
        .collect::<BTreeSet<_>>();
    if !explicit_requests.is_empty() {
        conflicting_speakers.retain(|speaker| !explicit_requests.contains(speaker));
        update_speaker_name_proposals(paths, |store| {
            store.pending_conflicts.retain(|conflict| {
                !explicit_requests
                    .contains(&(conflict.registry_id.clone(), conflict.speaker_id.clone()))
            });
            Ok(())
        })?;
    }
    let mut conflicts = Vec::new();
    if !conflicting_speakers.is_empty() {
        for (registry_id, speaker_id) in &conflicting_speakers {
            let mut options = verified_candidates
                .iter()
                .filter(|candidate| {
                    candidate.source == SpeakerNameProposalSource::Inferred
                        && candidate.registry_id == *registry_id
                        && candidate.speaker_id == *speaker_id
                })
                .map(conflict_option_from_verified)
                .collect::<Vec<_>>();
            if let Some(conflict) =
                persist_speaker_name_conflict(paths, registry_id, speaker_id, &mut options, now)?
            {
                conflicts.push(conflict);
            }
        }
    }
    for verified in &verified_candidates {
        let speaker_key = (verified.registry_id.clone(), verified.speaker_id.clone());
        if verified.source != SpeakerNameProposalSource::Inferred
            || conflicting_speakers.contains(&speaker_key)
        {
            continue;
        }
        let mut options = vec![conflict_option_from_verified(verified)];
        let conflict = persist_conflicting_pending_proposal(
            paths,
            &verified.registry_id,
            &verified.speaker_id,
            &verified.name,
            &mut options,
            now,
        )?;
        if let Some(conflict) = conflict {
            conflicting_speakers.insert(speaker_key);
            conflicts.push(conflict);
        }
    }

    let mut accepted = Vec::new();
    let mut handled_candidates = BTreeSet::<(String, String, String)>::new();
    for verified in verified_candidates {
        let speaker_key = (verified.registry_id.clone(), verified.speaker_id.clone());
        if conflicting_speakers.contains(&speaker_key)
            || !handled_candidates.insert((
                speaker_key.0.clone(),
                speaker_key.1.clone(),
                verified.name.clone(),
            ))
        {
            continue;
        }
        let request_fingerprint = proposal_fingerprint(
            &verified.registry_id,
            &verified.speaker_id,
            &verified.name,
            &verified.source_user_message_ids,
            &verified.evidence,
        )?;
        let duplicate = update_speaker_name_proposals(paths, |store| {
            let existing = store
                .proposals
                .iter()
                .find(|proposal| proposal.request_fingerprint == request_fingerprint);
            if let Some(existing) = existing {
                let unexpired = DateTime::parse_from_rfc3339(&existing.expires_at)
                    .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > now);
                if existing.status == SpeakerNameProposalStatus::Proposed && unexpired {
                    return Ok(Some(existing.clone()));
                }
                if !(verified.source == SpeakerNameProposalSource::Inferred
                    && verified.reconsidered
                    && existing.status == SpeakerNameProposalStatus::Rejected)
                {
                    return Ok(None);
                }
            }
            if verified.source == SpeakerNameProposalSource::Inferred {
                let suppression_active = store.proposals.iter().any(|proposal| {
                    proposal.source == SpeakerNameProposalSource::Inferred
                        && proposal.status == SpeakerNameProposalStatus::Rejected
                        && proposal.registry_id == verified.registry_id
                        && proposal.speaker_id == verified.speaker_id
                        && proposal.inferred_candidate_name.as_deref()
                            == Some(verified.name.as_str())
                        && proposal.suppressed_until.as_deref().is_some_and(|until| {
                            DateTime::parse_from_rfc3339(until)
                                .is_ok_and(|until| until.with_timezone(&Utc) > now)
                        })
                });
                if suppression_active && !verified.reconsidered {
                    return Ok(None);
                }
                let mut has_conflicting_pending = false;
                for proposal in &mut store.proposals {
                    if proposal.registry_id != verified.registry_id
                        || proposal.speaker_id != verified.speaker_id
                    {
                        continue;
                    }
                    if proposal.source == SpeakerNameProposalSource::UserRequest
                        && matches!(
                            proposal.status,
                            SpeakerNameProposalStatus::Proposed
                                | SpeakerNameProposalStatus::Applying
                                | SpeakerNameProposalStatus::Undoing
                        )
                    {
                        has_conflicting_pending = true;
                        continue;
                    }
                    if proposal.source != SpeakerNameProposalSource::Inferred
                        || proposal.name == verified.name
                    {
                        continue;
                    }
                    match proposal.status {
                        SpeakerNameProposalStatus::Proposed => {
                            proposal.status = SpeakerNameProposalStatus::Conflict;
                            has_conflicting_pending = true;
                        }
                        SpeakerNameProposalStatus::Applying => has_conflicting_pending = true,
                        _ => {}
                    }
                }
                if has_conflicting_pending {
                    return Ok(None);
                }
                if verified.reconsidered {
                    for proposal in &mut store.proposals {
                        if proposal.source == SpeakerNameProposalSource::Inferred
                            && proposal.status == SpeakerNameProposalStatus::Rejected
                            && proposal.registry_id == verified.registry_id
                            && proposal.speaker_id == verified.speaker_id
                            && proposal.inferred_candidate_name.as_deref()
                                == Some(verified.name.as_str())
                        {
                            proposal.suppressed_until = None;
                        }
                    }
                }
            }
            if verified.source == SpeakerNameProposalSource::UserRequest {
                for proposal in &mut store.proposals {
                    if proposal.source == SpeakerNameProposalSource::Inferred
                        && proposal.status == SpeakerNameProposalStatus::Rejected
                        && proposal.registry_id == verified.registry_id
                        && proposal.speaker_id == verified.speaker_id
                        && proposal.inferred_candidate_name.as_deref()
                            == Some(verified.name.as_str())
                    {
                        proposal.suppressed_until = None;
                    }
                }
            }
            let current_name = crate::speaker_names::load_speaker_names(paths, active_registry_id)?
                .remove(&verified.speaker_id);
            if verified.source == SpeakerNameProposalSource::Inferred && current_name.is_some() {
                return Ok(None);
            }
            if current_name.as_deref() == Some(verified.name.as_str()) {
                return Ok(None);
            }
            ensure_proposal_capacity(store)?;
            let inferred_candidate_name = (verified.source == SpeakerNameProposalSource::Inferred)
                .then(|| verified.name.clone());
            let proposal = StoredSpeakerNameProposal {
                id: Uuid::new_v4().to_string(),
                version: 1,
                registry_id: verified.registry_id,
                speaker_id: verified.speaker_id,
                current_name,
                name: verified.name,
                name_edited_by_user: false,
                source: verified.source,
                inferred_candidate_name,
                source_user_message_ids: verified.source_user_message_ids,
                request_fingerprint,
                evidence: verified.evidence,
                evidence_transcript_paths: verified.evidence_transcript_paths,
                authorized_transcript_paths: verified.authorized_transcript_paths,
                status: SpeakerNameProposalStatus::Proposed,
                created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                expires_at: (now + PROPOSAL_TTL)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                confirmation_bubble_id: None,
                pending_bubble_dismissal_id: None,
                suppressed_until: None,
                operation: None,
                completion_record: None,
                undo_completion_record: None,
                undo_card_status: SpeakerNameProposalUndoCardStatus::LegacyUnknown,
            };
            store.proposals.insert(0, proposal.clone());
            Ok(Some(proposal))
        })?;
        if let Some(proposal) = duplicate {
            accepted.push(proposal);
        }
    }
    Ok(SpeakerNameProposalAcceptance {
        proposals: accepted,
        conflicts,
    })
}

fn conflict_option_from_verified(
    candidate: &VerifiedCandidate,
) -> PendingSpeakerNameConflictOption {
    PendingSpeakerNameConflictOption {
        name: candidate.name.clone(),
        source_user_message_ids: candidate.source_user_message_ids.clone(),
        evidence: candidate.evidence.clone(),
        evidence_transcript_paths: candidate.evidence_transcript_paths.clone(),
        authorized_transcript_paths: candidate.authorized_transcript_paths.clone(),
    }
}

fn conflict_option_from_proposal(
    proposal: &StoredSpeakerNameProposal,
) -> PendingSpeakerNameConflictOption {
    PendingSpeakerNameConflictOption {
        name: proposal.name.clone(),
        source_user_message_ids: proposal.source_user_message_ids.clone(),
        evidence: proposal.evidence.clone(),
        evidence_transcript_paths: proposal.evidence_transcript_paths.clone(),
        authorized_transcript_paths: proposal.authorized_transcript_paths.clone(),
    }
}

fn merge_conflict_option(
    options: &mut Vec<PendingSpeakerNameConflictOption>,
    mut addition: PendingSpeakerNameConflictOption,
) {
    if let Some(existing) = options
        .iter_mut()
        .find(|option| option.name == addition.name)
    {
        for message_id in addition.source_user_message_ids.drain(..) {
            if !existing.source_user_message_ids.contains(&message_id) {
                existing.source_user_message_ids.push(message_id);
            }
        }
        for (evidence, path) in addition
            .evidence
            .drain(..)
            .zip(addition.evidence_transcript_paths.drain(..))
        {
            if !existing.evidence.contains(&evidence) && existing.evidence.len() < MAX_EVIDENCE {
                existing.evidence.push(evidence);
                existing.evidence_transcript_paths.push(path);
            }
        }
        for path in addition.authorized_transcript_paths.drain(..) {
            if !existing.authorized_transcript_paths.contains(&path)
                && existing.authorized_transcript_paths.len() < 3
            {
                existing.authorized_transcript_paths.push(path);
            }
        }
    } else if options.len() < MAX_CONFLICT_OPTIONS {
        options.push(addition);
    }
}

fn conflict_projection(conflict: &PendingSpeakerNameConflict) -> SpeakerNameProposalConflict {
    SpeakerNameProposalConflict {
        options: conflict.options.clone(),
    }
}

fn create_pending_conflict(
    registry_id: &str,
    speaker_id: &str,
    options: &mut [PendingSpeakerNameConflictOption],
    now: DateTime<Utc>,
) -> Option<PendingSpeakerNameConflict> {
    options.sort_by(|left, right| left.name.cmp(&right.name));
    if options.len() < 2 {
        return None;
    }
    Some(PendingSpeakerNameConflict {
        id: Uuid::new_v4().to_string(),
        registry_id: registry_id.to_owned(),
        speaker_id: speaker_id.to_owned(),
        options: options.to_vec(),
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        expires_at: (now + PENDING_CONFLICT_TTL)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}

fn persist_speaker_name_conflict(
    paths: &ConfigPaths,
    registry_id: &str,
    speaker_id: &str,
    options: &mut Vec<PendingSpeakerNameConflictOption>,
    now: DateTime<Utc>,
) -> Result<Option<SpeakerNameProposalConflict>, PersistenceError> {
    update_speaker_name_proposals(paths, |store| {
        prune_expired_pending_conflicts(store, now);
        if store.proposals.iter().any(|proposal| {
            proposal.registry_id == registry_id
                && proposal.speaker_id == speaker_id
                && proposal.source == SpeakerNameProposalSource::Inferred
                && proposal.status == SpeakerNameProposalStatus::Applying
        }) {
            return Ok(None);
        }
        for proposal in &mut store.proposals {
            if proposal.registry_id == registry_id
                && proposal.speaker_id == speaker_id
                && proposal.source == SpeakerNameProposalSource::Inferred
                && proposal.status == SpeakerNameProposalStatus::Proposed
            {
                merge_conflict_option(options, conflict_option_from_proposal(proposal));
                proposal.status = SpeakerNameProposalStatus::Conflict;
            }
        }
        let pending_index = store.pending_conflicts.iter().position(|conflict| {
            conflict.registry_id == registry_id && conflict.speaker_id == speaker_id
        });
        if let Some(index) = pending_index {
            let existing = store.pending_conflicts.remove(index);
            for option in existing.options {
                merge_conflict_option(options, option);
            }
        } else if store.pending_conflicts.len() >= MAX_PENDING_CONFLICTS {
            return Err(PersistenceError::Invalid(
                "話者名候補の競合確認を保存できる空き容量がありません".to_owned(),
            ));
        }
        let Some(conflict) = create_pending_conflict(registry_id, speaker_id, options, now) else {
            return Ok(None);
        };
        let projection = conflict_projection(&conflict);
        store.pending_conflicts.push(conflict);
        Ok(Some(projection))
    })
}

fn persist_conflicting_pending_proposal(
    paths: &ConfigPaths,
    registry_id: &str,
    speaker_id: &str,
    new_name: &str,
    options: &mut Vec<PendingSpeakerNameConflictOption>,
    now: DateTime<Utc>,
) -> Result<Option<SpeakerNameProposalConflict>, PersistenceError> {
    update_speaker_name_proposals(paths, |store| {
        prune_expired_pending_conflicts(store, now);
        if let Some(existing) = store.pending_conflicts.iter().find(|conflict| {
            conflict.registry_id == registry_id && conflict.speaker_id == speaker_id
        }) {
            return Ok(Some(conflict_projection(existing)));
        }
        let applying = store.proposals.iter().any(|proposal| {
            proposal.registry_id == registry_id
                && proposal.speaker_id == speaker_id
                && proposal.source == SpeakerNameProposalSource::Inferred
                && proposal.status == SpeakerNameProposalStatus::Applying
        });
        if applying {
            return Ok(None);
        }
        let Some(existing_index) = store.proposals.iter().position(|proposal| {
            proposal.registry_id == registry_id
                && proposal.speaker_id == speaker_id
                && proposal.source == SpeakerNameProposalSource::Inferred
                && proposal.status == SpeakerNameProposalStatus::Proposed
                && proposal.name != new_name
        }) else {
            return Ok(None);
        };
        if store.pending_conflicts.len() >= MAX_PENDING_CONFLICTS {
            return Err(PersistenceError::Invalid(
                "話者名候補の競合確認を保存できる空き容量がありません".to_owned(),
            ));
        }
        let existing = &mut store.proposals[existing_index];
        merge_conflict_option(options, conflict_option_from_proposal(existing));
        existing.status = SpeakerNameProposalStatus::Conflict;
        let Some(conflict) = create_pending_conflict(registry_id, speaker_id, options, now) else {
            return Ok(None);
        };
        let projection = conflict_projection(&conflict);
        store.pending_conflicts.push(conflict);
        Ok(Some(projection))
    })
}

fn selected_conflict_option(
    message: &str,
    options: &[PendingSpeakerNameConflictOption],
) -> Option<usize> {
    let normalized = message
        .trim()
        .trim_end_matches(['。', '！', '!', '？', '?', ' ', '\t', '\r', '\n']);
    let exact = options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            let accepted = [
                option.name.clone(),
                format!("{}さん", option.name),
                format!("{}氏", option.name),
                format!("{}さんで", option.name),
                format!("{}氏で", option.name),
            ];
            accepted
                .iter()
                .any(|accepted| accepted == normalized)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if exact.len() == 1 {
        return exact.first().copied();
    }

    let selected = split_conflict_selection_clauses(message)
        .into_iter()
        .filter(|clause| has_conflict_selection_language(clause))
        .filter(|clause| !has_conflict_selection_negation(clause))
        .flat_map(|clause| {
            options
                .iter()
                .enumerate()
                .filter_map(move |(index, option)| clause.contains(&option.name).then_some(index))
        })
        .collect::<BTreeSet<_>>();
    (selected.len() == 1).then(|| *selected.first().expect("one selected option"))
}

fn split_conflict_selection_clauses(message: &str) -> Vec<&str> {
    let mut clauses = Vec::new();
    for sentence in message.split(|character| {
        matches!(
            character,
            '。' | '！' | '!' | '？' | '?' | '\r' | '\n' | '、' | '，' | ',' | '；' | ';'
        )
    }) {
        let mut remaining = sentence;
        while let Some((index, connective)) = next_conflict_selection_connective(remaining) {
            clauses.push(&remaining[..index]);
            remaining = &remaining[index + connective.len()..];
        }
        clauses.push(remaining);
    }
    clauses
}

fn next_conflict_selection_connective(clause: &str) -> Option<(usize, &'static str)> {
    const CONNECTIVES: [&str; 7] = ["けれど", "けど", "けれども", "ので", "から", "が", "し"];

    CONNECTIVES
        .iter()
        .flat_map(|connective| {
            clause
                .match_indices(connective)
                .map(move |(index, _)| (index, *connective))
        })
        .filter(|(index, connective)| {
            let prefix = &clause[..*index];
            let suffix = &clause[index + connective.len()..];
            match *connective {
                "が" => !suffix.starts_with("いい") && !suffix.starts_with("良い"),
                "し" => !prefix.ends_with("を候補に"),
                _ => true,
            }
        })
        .min_by_key(|(index, _)| *index)
}

fn has_conflict_selection_negation(clause: &str) -> bool {
    let normalized = clause.to_lowercase();
    [
        "ない",
        "ません",
        "しないで",
        "以外",
        "じゃなく",
        "ではなく",
        "not",
        "don't",
        "don’t",
        "do not",
        "never",
        "cannot",
        "can't",
        "can’t",
        "won't",
        "won’t",
        "except",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        || ["を選ぶな", "を候補にするな", "を候補にしな"]
            .iter()
            .any(|marker| clause.contains(marker))
}

fn conflict_selection_attempt_mentions_option(
    message: &str,
    options: &[PendingSpeakerNameConflictOption],
) -> bool {
    options.iter().any(|option| message.contains(&option.name))
        && has_conflict_selection_language(message)
}

fn has_conflict_selection_language(message: &str) -> bool {
    [
        "にします",
        "に決めます",
        "でお願いします",
        "の方",
        "のほう",
        "がいい",
        "が良い",
        "を選び",
        "を選ぶ",
        "を候補に",
        "にして",
        "を希望",
        "choose",
        "select",
        "prefer",
        "go with",
    ]
    .iter()
    .any(|marker| message.to_lowercase().contains(&marker.to_lowercase()))
}

/// 競合候補を利用者が選んだ場合、保存済みの検証済み根拠から一件の提案を作る。
pub fn resolve_pending_speaker_name_conflict(
    paths: &ConfigPaths,
    user_messages: &[(String, String)],
    active_registry_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<StoredSpeakerNameProposal>, PersistenceError> {
    if user_messages.is_empty() || Uuid::parse_str(active_registry_id).is_err() {
        return Ok(Vec::new());
    }
    let current_names = crate::speaker_names::load_speaker_names(paths, active_registry_id)?;
    update_speaker_name_proposals(paths, |store| {
        prune_expired_pending_conflicts(store, now);
        let selections = store
            .pending_conflicts
            .iter()
            .enumerate()
            .filter(|(_, conflict)| conflict.registry_id == active_registry_id)
            .filter_map(|(conflict_index, conflict)| {
                let matches = user_messages
                    .iter()
                    .filter_map(|(_, message)| {
                        selected_conflict_option(message, &conflict.options)
                            .map(|option_index| (conflict_index, option_index))
                    })
                    .collect::<BTreeSet<_>>();
                (matches.len() == 1).then(|| *matches.first().expect("one selection"))
            })
            .collect::<Vec<_>>();
        if selections.len() != 1 {
            return Ok(Vec::new());
        }
        let (conflict_index, option_index) = selections[0];
        let conflict = store.pending_conflicts.remove(conflict_index);
        if current_names.contains_key(&conflict.speaker_id) {
            return Ok(Vec::new());
        }
        let option = conflict.options[option_index].clone();
        let fingerprint = proposal_fingerprint(
            &conflict.registry_id,
            &conflict.speaker_id,
            &option.name,
            &option.source_user_message_ids,
            &option.evidence,
        )?;
        if let Some(existing) = store.proposals.iter().find(|proposal| {
            proposal.request_fingerprint == fingerprint
                && proposal.status == SpeakerNameProposalStatus::Proposed
                && DateTime::parse_from_rfc3339(&proposal.expires_at)
                    .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > now)
        }) {
            return Ok(vec![existing.clone()]);
        }
        ensure_proposal_capacity(store)?;
        let proposal = StoredSpeakerNameProposal {
            id: Uuid::new_v4().to_string(),
            version: 1,
            registry_id: conflict.registry_id,
            speaker_id: conflict.speaker_id,
            current_name: None,
            name: option.name.clone(),
            name_edited_by_user: false,
            source: SpeakerNameProposalSource::Inferred,
            inferred_candidate_name: Some(option.name),
            source_user_message_ids: option.source_user_message_ids,
            request_fingerprint: fingerprint,
            evidence: option.evidence,
            evidence_transcript_paths: option.evidence_transcript_paths,
            authorized_transcript_paths: option.authorized_transcript_paths,
            status: SpeakerNameProposalStatus::Proposed,
            created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            expires_at: (now + PROPOSAL_TTL).to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            confirmation_bubble_id: None,
            pending_bubble_dismissal_id: None,
            suppressed_until: None,
            operation: None,
            completion_record: None,
            undo_completion_record: None,
            undo_card_status: SpeakerNameProposalUndoCardStatus::LegacyUnknown,
        };
        store.proposals.insert(0, proposal.clone());
        Ok(vec![proposal])
    })
}

fn prune_expired_pending_conflicts(store: &mut SpeakerNameProposalStore, now: DateTime<Utc>) {
    store.pending_conflicts.retain(|conflict| {
        DateTime::parse_from_rfc3339(&conflict.expires_at)
            .is_ok_and(|expires_at| expires_at.with_timezone(&Utc) > now)
    });
}

#[derive(Debug, Clone)]
struct VerifiedCandidate {
    registry_id: String,
    speaker_id: String,
    name: String,
    source: SpeakerNameProposalSource,
    reconsidered: bool,
    source_user_message_ids: Vec<String>,
    evidence: Vec<SpeakerNameProposalEvidence>,
    evidence_transcript_paths: Vec<String>,
    authorized_transcript_paths: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn verify_candidate(
    paths: &ConfigPaths,
    candidate: &SpeakerNameProposalCandidate,
    user_ids: &BTreeSet<&str>,
    user_by_id: &std::collections::BTreeMap<&str, &str>,
    records: &[TranscriptRecord],
    records_by_path: &std::collections::BTreeMap<PathBuf, Vec<TranscriptRecord>>,
    allowed_ref_paths: &std::collections::BTreeMap<(String, u64, u64), PathBuf>,
    allowed_paths: &std::collections::BTreeMap<PathBuf, ()>,
    resolver: &PromptSpeakerIdResolver,
    active_registry_id: &str,
) -> Result<Option<VerifiedCandidate>, PersistenceError> {
    let Ok(candidate_name) = validate_speaker_name(&candidate.name) else {
        return Ok(None);
    };
    if candidate_name != candidate.name
        || candidate.source_user_message_ids.is_empty()
        || candidate.source_user_message_ids.len() > 5
        || candidate.evidence.is_empty()
        || candidate.evidence.len() > MAX_EVIDENCE
        || candidate
            .source_user_message_ids
            .iter()
            .any(|id| !user_ids.contains(id.as_str()))
        || candidate
            .source_user_message_ids
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != candidate.source_user_message_ids.len()
    {
        return Ok(None);
    }
    let source_messages = candidate
        .source_user_message_ids
        .iter()
        .filter_map(|id| user_by_id.get(id.as_str()).copied())
        .collect::<Vec<_>>();
    if source_messages.len() != candidate.source_user_message_ids.len() {
        return Ok(None);
    }
    let requested_names = source_messages
        .iter()
        .map(|message| requested_display_name(message, &candidate_name))
        .collect::<Option<BTreeSet<_>>>();
    let direct_name = requested_names
        .filter(|names| names.len() == 1)
        .and_then(|mut names| names.pop_first());
    let (source, name) = if let Some(name) = direct_name {
        if name != candidate_name && !name.starts_with(&candidate_name) {
            return Ok(None);
        }
        (SpeakerNameProposalSource::UserRequest, name)
    } else if source_messages
        .iter()
        .all(|message| user_message_requests_speaker_name_inference(message))
    {
        (SpeakerNameProposalSource::Inferred, candidate_name.clone())
    } else {
        return Ok(None);
    };
    let Ok(name) = validate_speaker_name(&name) else {
        return Ok(None);
    };
    let evidence_paths = candidate
        .evidence
        .iter()
        .map(|item| {
            let key = (
                item.observation_id.clone(),
                item.audio_start_ms,
                item.audio_end_ms,
            );
            allowed_ref_paths
                .get(&key)
                .filter(|path| valid_indexed_path(&paths.transcripts, path))
                .cloned()
        })
        .collect::<Option<Vec<_>>>();
    let Some(evidence_paths) = evidence_paths else {
        return Ok(None);
    };
    if !candidate.evidence.iter().all(valid_evidence_shape) {
        return Ok(None);
    }
    let (speaker_id, evidence_transcript_paths) = match source {
        SpeakerNameProposalSource::UserRequest => {
            if candidate
                .evidence
                .iter()
                .any(|item| item.role.is_some() || item.group_id.is_some())
            {
                return Ok(None);
            }
            let Some((speaker_id, registry_id)) = verify_user_request_evidence(
                candidate,
                &evidence_paths,
                records,
                records_by_path,
                allowed_ref_paths,
                resolver,
                active_registry_id,
            ) else {
                return Ok(None);
            };
            return Ok(Some(VerifiedCandidate {
                registry_id,
                speaker_id,
                name,
                source,
                reconsidered: false,
                source_user_message_ids: candidate.source_user_message_ids.clone(),
                evidence: candidate.evidence.clone(),
                evidence_transcript_paths: evidence_paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
                authorized_transcript_paths: allowed_paths
                    .keys()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            }));
        }
        SpeakerNameProposalSource::Inferred => {
            let Some(speaker_id) = verify_inferred_evidence(
                &name,
                &candidate.evidence,
                &evidence_paths,
                records,
                records_by_path,
                resolver,
                active_registry_id,
            ) else {
                return Ok(None);
            };
            if candidate.speaker_id != speaker_id {
                return Ok(None);
            }
            (
                speaker_id,
                evidence_paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
            )
        }
    };
    let registry_id = active_registry_id.to_owned();
    Ok(Some(VerifiedCandidate {
        registry_id,
        speaker_id,
        name,
        source,
        reconsidered: source_messages
            .iter()
            .any(|message| user_message_reconsiders_speaker_name_inference(message)),
        source_user_message_ids: candidate.source_user_message_ids.clone(),
        evidence: candidate.evidence.clone(),
        evidence_transcript_paths,
        authorized_transcript_paths: allowed_paths
            .keys()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    }))
}

fn valid_evidence_shape(evidence: &SpeakerNameProposalEvidence) -> bool {
    let valid_group = evidence.group_id.as_deref().is_none_or(|group_id| {
        !group_id.trim().is_empty()
            && group_id.chars().count() <= MAX_GROUP_ID_CHARS
            && !group_id.chars().any(char::is_control)
    });
    !evidence.observation_id.trim().is_empty()
        && evidence.audio_start_ms < evidence.audio_end_ms
        && !evidence.quote.trim().is_empty()
        && evidence.quote.chars().count() <= MAX_QUOTE_CHARS
        && !evidence.quote.chars().any(char::is_control)
        && valid_group
}

fn inferred_evidence_role_shape(evidence: &[SpeakerNameProposalEvidence]) -> bool {
    let self_introductions = evidence
        .iter()
        .filter(|item| item.role == Some(SpeakerNameEvidenceRole::SelfIntroduction))
        .collect::<Vec<_>>();
    let addresses = evidence
        .iter()
        .filter(|item| item.role == Some(SpeakerNameEvidenceRole::Address))
        .collect::<Vec<_>>();
    let responses = evidence
        .iter()
        .filter(|item| item.role == Some(SpeakerNameEvidenceRole::Response))
        .collect::<Vec<_>>();
    let third_party = evidence
        .iter()
        .filter(|item| item.role == Some(SpeakerNameEvidenceRole::ThirdPartyMention))
        .collect::<Vec<_>>();
    if third_party.iter().any(|item| item.group_id.is_some())
        || self_introductions
            .iter()
            .any(|item| item.group_id.is_some())
    {
        return false;
    }
    if self_introductions.len() == 1 && addresses.is_empty() && responses.is_empty() {
        return true;
    }
    matches!((addresses.as_slice(), responses.as_slice()), ([_], [_]))
        && addresses[0].observation_id == responses[0].observation_id
        && addresses[0].group_id.is_some()
        && addresses[0].group_id == responses[0].group_id
        && self_introductions.is_empty()
}

fn verify_user_request_evidence(
    candidate: &SpeakerNameProposalCandidate,
    evidence_paths: &[PathBuf],
    records: &[TranscriptRecord],
    records_by_path: &std::collections::BTreeMap<PathBuf, Vec<TranscriptRecord>>,
    allowed_ref_paths: &std::collections::BTreeMap<(String, u64, u64), PathBuf>,
    resolver: &PromptSpeakerIdResolver,
    active_registry_id: &str,
) -> Option<(String, String)> {
    let mut resolved_ids = BTreeSet::new();
    let mut registry_id = None;
    for (item, evidence_path) in candidate.evidence.iter().zip(evidence_paths) {
        let matching = records_by_path
            .get(evidence_path)?
            .iter()
            .filter(|record| {
                record.observation_id == item.observation_id
                    && record.audio_start_ms == Some(item.audio_start_ms)
                    && record.audio_end_ms == Some(item.audio_end_ms)
                    && record.source == "speaker"
                    && record.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                    && record.text.contains(&item.quote)
                    && record.speaker_registry_id.as_deref() == Some(active_registry_id)
                    && allowed_ref_paths.get(&(
                        record.observation_id.clone(),
                        item.audio_start_ms,
                        item.audio_end_ms,
                    )) == Some(evidence_path)
            })
            .filter_map(|record| {
                let registry = record.speaker_registry_id.as_deref()?;
                let resolved = resolver.resolve(registry, record.speaker_tag.as_deref()?)?;
                Some((
                    registry.to_owned(),
                    resolved.canonical_id,
                    resolved.prompt_id,
                ))
            })
            .collect::<Vec<_>>();
        let unique_ids = matching
            .iter()
            .map(|(_, _, prompt_id)| prompt_id.clone())
            .collect::<BTreeSet<_>>();
        if unique_ids.len() != 1 || !unique_ids.contains(&candidate.speaker_id) {
            return None;
        }
        let (matched_registry, canonical_id, _) = matching.first()?;
        if registry_id
            .as_deref()
            .is_some_and(|current| current != matched_registry)
        {
            return None;
        }
        registry_id = Some(matched_registry.clone());
        resolved_ids.insert(canonical_id.clone());

        let other_speakers = records
            .iter()
            .filter(|record| {
                record.source == "speaker"
                    && record.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                    && record.text.contains(&item.quote)
                    && record.speaker_registry_id.as_deref() == Some(active_registry_id)
                    && record
                        .audio_start_ms
                        .zip(record.audio_end_ms)
                        .is_some_and(|(start, end)| {
                            allowed_ref_paths.contains_key(&(
                                record.observation_id.clone(),
                                start,
                                end,
                            ))
                        })
            })
            .filter_map(|record| {
                resolver
                    .resolve(
                        record.speaker_registry_id.as_deref()?,
                        record.speaker_tag.as_deref()?,
                    )
                    .map(|resolved| resolved.prompt_id)
            })
            .collect::<BTreeSet<_>>();
        if other_speakers.len() > 1 {
            return None;
        }
    }
    if resolved_ids.len() != 1 {
        return None;
    }
    Some((resolved_ids.into_iter().next().unwrap(), registry_id?))
}

fn verify_inferred_evidence(
    name: &str,
    evidence: &[SpeakerNameProposalEvidence],
    evidence_paths: &[PathBuf],
    records: &[TranscriptRecord],
    records_by_path: &std::collections::BTreeMap<PathBuf, Vec<TranscriptRecord>>,
    resolver: &PromptSpeakerIdResolver,
    active_registry_id: &str,
) -> Option<String> {
    if evidence.len() != evidence_paths.len()
        || evidence
            .iter()
            .map(|item| {
                (
                    item.observation_id.as_str(),
                    item.audio_start_ms,
                    item.audio_end_ms,
                )
            })
            .collect::<BTreeSet<_>>()
            .len()
            != evidence.len()
    {
        return None;
    }
    let mut self_introductions = Vec::new();
    let mut addresses = Vec::new();
    let mut responses = Vec::new();
    for (item, evidence_path) in evidence.iter().zip(evidence_paths) {
        let role = item.role?;
        match role {
            SpeakerNameEvidenceRole::SelfIntroduction => {
                if item.group_id.is_some() || !item.quote.contains(name) {
                    return None;
                }
            }
            SpeakerNameEvidenceRole::Address | SpeakerNameEvidenceRole::Response => {
                let group_id = item.group_id.as_deref()?;
                if group_id.trim().is_empty() || group_id.chars().count() > MAX_GROUP_ID_CHARS {
                    return None;
                }
                if role == SpeakerNameEvidenceRole::Address && !item.quote.contains(name) {
                    return None;
                }
            }
            SpeakerNameEvidenceRole::ThirdPartyMention => {
                if item.group_id.is_some() || !item.quote.contains(name) {
                    return None;
                }
            }
        }
        let path_records = records_by_path.get(evidence_path)?;
        let indexed_path_matches = path_records.iter().any(|record| {
            record.observation_id == item.observation_id
                && record.audio_start_ms == Some(item.audio_start_ms)
                && record.audio_end_ms == Some(item.audio_end_ms)
                && record.source == "speaker"
                && record.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                && record.text.contains(&item.quote)
                && record.speaker_registry_id.as_deref() == Some(active_registry_id)
        });
        if !indexed_path_matches {
            return None;
        }
        let matched_ref = path_records
            .iter()
            .filter(|record| {
                record.observation_id == item.observation_id
                    && record.audio_start_ms == Some(item.audio_start_ms)
                    && record.audio_end_ms == Some(item.audio_end_ms)
                    && record.source == "speaker"
                    && record.speaker_status == Some(SpeakerIdentificationStatus::Identified)
                    && record.text.contains(&item.quote)
                    && record.speaker_registry_id.as_deref() == Some(active_registry_id)
            })
            .filter_map(|record| {
                let resolved = resolver.resolve(
                    record.speaker_registry_id.as_deref()?,
                    record.speaker_tag.as_deref()?,
                )?;
                Some((record, resolved.canonical_id, resolved.prompt_id))
            })
            .collect::<Vec<_>>();
        let unique_prompt_ids = matched_ref
            .iter()
            .map(|(_, _, prompt_id)| prompt_id.clone())
            .collect::<BTreeSet<_>>();
        if unique_prompt_ids.len() != 1 {
            return None;
        }
        let target_id = unique_prompt_ids.into_iter().next()?;
        let target_record = matched_ref
            .iter()
            .find(|(_, _, prompt_id)| prompt_id == &target_id)?
            .0;
        match role {
            SpeakerNameEvidenceRole::SelfIntroduction => {
                self_introductions.push((item, target_record, target_id));
            }
            SpeakerNameEvidenceRole::Address => addresses.push((item, target_record, target_id)),
            SpeakerNameEvidenceRole::Response => responses.push((item, target_record, target_id)),
            SpeakerNameEvidenceRole::ThirdPartyMention => {}
        }
    }
    let target_id = match (
        self_introductions.as_slice(),
        addresses.as_slice(),
        responses.as_slice(),
    ) {
        ([(_item, _, speaker_id)], [], []) => speaker_id.clone(),
        (
            [],
            [(address, address_record, address_id)],
            [(response, response_record, response_id)],
        ) => {
            let address_group = address.group_id.as_deref()?;
            if response.group_id.as_deref() != Some(address_group)
                || address.observation_id != response.observation_id
                || address_id == response_id
                || address_record.audio_end_ms? > response_record.audio_start_ms?
                || response_record
                    .audio_start_ms?
                    .saturating_sub(address_record.audio_end_ms?)
                    > MAX_ADDRESS_RESPONSE_GAP_MS
            {
                return None;
            }
            if records.iter().any(|record| {
                record.observation_id == address.observation_id
                    && record.source == "speaker"
                    && (record.audio_start_ms.is_none() || record.audio_end_ms.is_none())
            }) {
                return None;
            }
            let mut same_observation = records
                .iter()
                .filter(|record| record.observation_id == address.observation_id)
                .filter_map(|record| Some((record.audio_start_ms?, record.audio_end_ms?, record)))
                .collect::<Vec<_>>();
            same_observation.sort_by_key(|(start, end, _)| (*start, *end));
            let address_index = same_observation.iter().position(|(_, _, record)| {
                record.observation_id == address_record.observation_id
                    && record.audio_start_ms == address_record.audio_start_ms
                    && record.audio_end_ms == address_record.audio_end_ms
            })?;
            let response_index = same_observation.iter().position(|(_, _, record)| {
                record.observation_id == response_record.observation_id
                    && record.audio_start_ms == response_record.audio_start_ms
                    && record.audio_end_ms == response_record.audio_end_ms
            })?;
            if response_index != address_index + 1 {
                return None;
            }
            response_id.clone()
        }
        _ => return None,
    };

    let root_text_supports_name = self_introductions
        .first()
        .is_some_and(|(item, _, _)| item.quote.contains(name))
        || addresses
            .first()
            .is_some_and(|(item, _, _)| item.quote.contains(name));
    if !root_text_supports_name {
        return None;
    }
    Some(target_id)
}

fn is_explicit_name_request(message: &str, name: &str) -> bool {
    requested_display_name(message, name).as_deref() == Some(name)
}

fn requested_display_name(message: &str, candidate_name: &str) -> Option<String> {
    if !user_message_requests_speaker_name(message) {
        return None;
    }
    message
        .match_indices(candidate_name)
        .find_map(|(name_index, matched)| {
            let before = &message[..name_index];
            let mut action_start = name_index + matched.len();
            let mut requested_name = candidate_name.to_owned();
            skip_quote_closers(message, &mut action_start);
            for honorific in ["さん", "くん", "ちゃん", "氏", "様", "先生"] {
                if message[action_start..].starts_with(honorific) {
                    requested_name.push_str(honorific);
                    action_start += honorific.len();
                    skip_quote_closers(message, &mut action_start);
                    break;
                }
            }
            let after = &message[action_start..];
            let terms = speaker_name_request_terms();
            if !has_unquoted_subject_marker(before) {
                return None;
            }
            terms.actions.iter().find_map(|action| {
                let action_end = action_start + action.len();
                if !after.starts_with(action)
                    || is_inside_quote(message, action_start)
                    || !has_direct_request_tail(&message[action_end..])
                {
                    return None;
                }
                Some(requested_name.clone())
            })
        })
}

fn skip_quote_closers(message: &str, byte_index: &mut usize) {
    while message[*byte_index..]
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '」' | '』' | '”' | '"'))
    {
        *byte_index += message[*byte_index..]
            .chars()
            .next()
            .expect("quote closer")
            .len_utf8();
    }
}

fn indexed_transcript_paths(
    paths: &ConfigPaths,
    audio_log_index: Option<&Value>,
    current_observation: Option<&Value>,
) -> Result<std::collections::BTreeMap<PathBuf, ()>, PersistenceError> {
    let mut result = std::collections::BTreeMap::new();
    if let Some(files) = audio_log_index
        .and_then(|index| index.get("files"))
        .and_then(Value::as_array)
    {
        for path in files
            .iter()
            .filter_map(|file| file.get("transcriptPath").and_then(Value::as_str))
        {
            let path = PathBuf::from(path);
            if !valid_indexed_path(&paths.transcripts, &path) {
                return Err(PersistenceError::Invalid(
                    "話者名提案の transcript path が許可範囲外です".to_owned(),
                ));
            }
            result.insert(path, ());
        }
    }
    if let Some(observation) = current_observation {
        let mut current_paths = observation
            .get("transcriptPath")
            .and_then(Value::as_str)
            .into_iter()
            .collect::<Vec<_>>();
        current_paths.extend(
            observation
                .get("audioSegments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|segment| segment.get("transcriptPath").and_then(Value::as_str)),
        );
        for path in current_paths {
            let path = PathBuf::from(path);
            if !valid_indexed_path(&paths.transcripts, &path) {
                return Err(PersistenceError::Invalid(
                    "話者名提案の transcript path が許可範囲外です".to_owned(),
                ));
            }
            result.insert(path, ());
        }
    }
    Ok(result)
}

fn indexed_transcript_ref_paths(
    paths: &ConfigPaths,
    audio_log_index: Option<&Value>,
    current_observation: Option<&Value>,
) -> Result<std::collections::BTreeMap<(String, u64, u64), PathBuf>, PersistenceError> {
    let mut result = std::collections::BTreeMap::new();
    if let Some(files) = audio_log_index
        .and_then(|index| index.get("files"))
        .and_then(Value::as_array)
    {
        for file in files {
            let Some(path) = file.get("transcriptPath").and_then(Value::as_str) else {
                continue;
            };
            let path = PathBuf::from(path);
            if !valid_indexed_path(&paths.transcripts, &path) {
                return Err(PersistenceError::Invalid(
                    "話者名提案の transcript path が許可範囲外です".to_owned(),
                ));
            }
            for reference in file
                .get("speakers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|speaker| speaker.get("references").and_then(Value::as_array))
                .flatten()
            {
                let (Some(observation_id), Some(start), Some(end)) = (
                    reference.get("observationId").and_then(Value::as_str),
                    reference.get("audioStartMs").and_then(Value::as_u64),
                    reference.get("audioEndMs").and_then(Value::as_u64),
                ) else {
                    continue;
                };
                result.insert((observation_id.to_owned(), start, end), path.clone());
            }
        }
    }
    if let Some(observation) = current_observation {
        let observation_id = observation
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if observation.get("source").and_then(Value::as_str) == Some("speaker")
            && observation.get("speakerStatus").and_then(Value::as_str) == Some("identified")
        {
            if let (Some(start), Some(end), Some(path)) = (
                observation.get("audioStartMs").and_then(Value::as_u64),
                observation.get("audioEndMs").and_then(Value::as_u64),
                observation.get("transcriptPath").and_then(Value::as_str),
            ) {
                let path = PathBuf::from(path);
                if !valid_indexed_path(&paths.transcripts, &path) {
                    return Err(PersistenceError::Invalid(
                        "話者名提案の transcript path が許可範囲外です".to_owned(),
                    ));
                }
                result.insert((observation_id.to_owned(), start, end), path);
            }
        }
        if let Some(segments) = observation.get("audioSegments").and_then(Value::as_array) {
            for segment in segments {
                let (Some(start), Some(end), Some(path)) = (
                    segment.get("audioStartMs").and_then(Value::as_u64),
                    segment.get("audioEndMs").and_then(Value::as_u64),
                    segment.get("transcriptPath").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let path = PathBuf::from(path);
                if !valid_indexed_path(&paths.transcripts, &path) {
                    return Err(PersistenceError::Invalid(
                        "話者名提案の transcript path が許可範囲外です".to_owned(),
                    ));
                }
                let id = segment
                    .get("observationId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        crate::state::audio_segment_observation_id(
                            segment
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or(observation_id),
                        )
                    });
                result.insert((id.to_owned(), start, end), path);
            }
        }
    }
    Ok(result)
}

fn valid_indexed_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).ok().is_some_and(|relative| {
        relative.components().count() == 1
            && relative
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.ends_with(".jsonl")
                        && chrono::NaiveDate::parse_from_str(
                            name.trim_end_matches(".jsonl"),
                            "%Y-%m-%d",
                        )
                        .is_ok()
                })
    })
}

fn validate_store(store: &SpeakerNameProposalStore) -> Result<(), PersistenceError> {
    if store.schema_version != SCHEMA_VERSION
        || store.proposals.len() > MAX_PROPOSALS
        || store.pending_conflicts.len() > MAX_PENDING_CONFLICTS
    {
        return Err(PersistenceError::Invalid(
            "話者名提案ストアの schemaVersion または件数が不正です".to_owned(),
        ));
    }
    let mut ids = BTreeSet::new();
    for proposal in &store.proposals {
        let unresolved = matches!(
            proposal.status,
            SpeakerNameProposalStatus::Proposed
                | SpeakerNameProposalStatus::Applying
                | SpeakerNameProposalStatus::Undoing
        );
        let evidence_invalid = if unresolved {
            proposal.evidence.is_empty()
                || proposal.evidence_transcript_paths.len() != proposal.evidence.len()
                || proposal.authorized_transcript_paths.is_empty()
                || proposal.authorized_transcript_paths.len() > 3
                || proposal.source_user_message_ids.is_empty()
                || proposal.source_user_message_ids.len() > 5
        } else {
            !proposal.evidence.is_empty()
                || !proposal.evidence_transcript_paths.is_empty()
                || !proposal.authorized_transcript_paths.is_empty()
                || !proposal.source_user_message_ids.is_empty()
        };
        let source_evidence_invalid = unresolved
            && match proposal.source {
                SpeakerNameProposalSource::UserRequest => {
                    proposal
                        .evidence
                        .iter()
                        .any(|evidence| evidence.role.is_some() || evidence.group_id.is_some())
                        || proposal.inferred_candidate_name.is_some()
                }
                SpeakerNameProposalSource::Inferred => {
                    proposal.current_name.is_some()
                        || proposal
                            .inferred_candidate_name
                            .as_deref()
                            .is_none_or(|name| {
                                validate_speaker_name(name).as_deref() != Ok(name)
                                    || (!proposal.name_edited_by_user && name != proposal.name)
                            })
                        || !inferred_evidence_role_shape(&proposal.evidence)
                }
            };
        if Uuid::parse_str(&proposal.id).is_err()
            || !ids.insert(proposal.id.as_str())
            || proposal.version == 0
            || Uuid::parse_str(&proposal.registry_id).is_err()
            || !crate::speaker_id::is_valid_speaker_id(&proposal.speaker_id)
            || validate_speaker_name(&proposal.name).as_deref() != Ok(proposal.name.as_str())
            || proposal
                .current_name
                .as_deref()
                .is_some_and(|name| validate_speaker_name(name).as_deref() != Ok(name))
            || evidence_invalid
            || source_evidence_invalid
            || proposal.suppressed_until.as_deref().is_some_and(|until| {
                proposal.source != SpeakerNameProposalSource::Inferred
                    || proposal.status != SpeakerNameProposalStatus::Rejected
                    || DateTime::parse_from_rfc3339(until).is_err()
            })
            || proposal.evidence.len() > MAX_EVIDENCE
            || DateTime::parse_from_rfc3339(&proposal.created_at).is_err()
            || DateTime::parse_from_rfc3339(&proposal.expires_at).is_err()
            || !valid_result_record(proposal.completion_record.as_ref())
            || !valid_result_record(proposal.undo_completion_record.as_ref())
            || proposal.evidence.iter().any(|evidence| {
                evidence.observation_id.trim().is_empty()
                    || evidence.audio_start_ms >= evidence.audio_end_ms
                    || evidence.quote.trim().is_empty()
                    || evidence.quote.chars().count() > MAX_QUOTE_CHARS
                    || evidence.quote.chars().any(char::is_control)
            })
            || proposal
                .source_user_message_ids
                .iter()
                .any(|id| id.trim().is_empty())
            || (!proposal.request_fingerprint.is_empty()
                && (proposal.request_fingerprint.len() != 64
                    || !proposal
                        .request_fingerprint
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())))
            || proposal
                .confirmation_bubble_id
                .as_deref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
            || proposal
                .pending_bubble_dismissal_id
                .as_deref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err(PersistenceError::Invalid(
                "話者名提案ストアのレコードが不正です".to_owned(),
            ));
        }
        if let Some(operation) = &proposal.operation {
            if Uuid::parse_str(&operation.id).is_err()
                || operation.after_name != proposal.name
                || validate_speaker_name(&operation.after_name).as_deref()
                    != Ok(operation.after_name.as_str())
                || operation
                    .before_name
                    .as_deref()
                    .is_some_and(|name| validate_speaker_name(name).as_deref() != Ok(name))
                || operation
                    .undo_id
                    .as_deref()
                    .is_some_and(|id| Uuid::parse_str(id).is_err())
            {
                return Err(PersistenceError::Invalid(
                    "話者名提案の操作記録が不正です".to_owned(),
                ));
            }
        }
        if matches!(
            proposal.status,
            SpeakerNameProposalStatus::Applying
                | SpeakerNameProposalStatus::Applied
                | SpeakerNameProposalStatus::Undoing
                | SpeakerNameProposalStatus::Undone
        ) && proposal.operation.is_none()
        {
            return Err(PersistenceError::Invalid(
                "話者名提案の操作記録がありません".to_owned(),
            ));
        }
    }
    let mut conflict_ids = BTreeSet::new();
    let mut conflict_speakers = BTreeSet::new();
    for conflict in &store.pending_conflicts {
        let speaker_key = (conflict.registry_id.as_str(), conflict.speaker_id.as_str());
        let names = conflict
            .options
            .iter()
            .map(|option| option.name.as_str())
            .collect::<BTreeSet<_>>();
        if Uuid::parse_str(&conflict.id).is_err()
            || !conflict_ids.insert(conflict.id.as_str())
            || Uuid::parse_str(&conflict.registry_id).is_err()
            || !crate::speaker_id::is_valid_speaker_id(&conflict.speaker_id)
            || !conflict_speakers.insert(speaker_key)
            || conflict.options.len() < 2
            || conflict.options.len() > MAX_CONFLICT_OPTIONS
            || names.len() != conflict.options.len()
            || DateTime::parse_from_rfc3339(&conflict.created_at).is_err()
            || DateTime::parse_from_rfc3339(&conflict.expires_at).is_err()
            || conflict.options.iter().any(|option| {
                validate_speaker_name(&option.name).as_deref() != Ok(option.name.as_str())
                    || option.source_user_message_ids.is_empty()
                    || option.source_user_message_ids.len() > 5
                    || option.evidence.is_empty()
                    || option.evidence.len() > MAX_EVIDENCE
                    || option.evidence_transcript_paths.len() != option.evidence.len()
                    || option.authorized_transcript_paths.is_empty()
                    || option.authorized_transcript_paths.len() > 3
                    || option
                        .source_user_message_ids
                        .iter()
                        .any(|id| id.trim().is_empty())
                    || option.evidence.iter().any(|evidence| {
                        evidence.observation_id.trim().is_empty()
                            || evidence.audio_start_ms >= evidence.audio_end_ms
                            || evidence.quote.trim().is_empty()
                            || evidence.quote.chars().count() > MAX_QUOTE_CHARS
                            || evidence.quote.chars().any(char::is_control)
                    })
            })
        {
            return Err(PersistenceError::Invalid(
                "話者名候補の競合確認レコードが不正です".to_owned(),
            ));
        }
    }
    Ok(())
}

/// 承認直前に依頼元、許可済み transcript、期間、引用、resolver を再検証する。
pub fn verify_stored_speaker_name_proposal(
    paths: &ConfigPaths,
    proposal: &StoredSpeakerNameProposal,
    source_user_messages: &[(String, String)],
    resolver: &PromptSpeakerIdResolver,
    active_registry_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, PersistenceError> {
    if proposal.registry_id != active_registry_id
        || Uuid::parse_str(active_registry_id).is_err()
        || DateTime::parse_from_rfc3339(&proposal.expires_at)
            .ok()
            .is_none_or(|expires_at| expires_at.with_timezone(&Utc) <= now)
        || proposal.evidence.len() != proposal.evidence_transcript_paths.len()
    {
        return Ok(false);
    }
    let user_by_id = source_user_messages
        .iter()
        .map(|(id, message)| (id.as_str(), message.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    for source_id in &proposal.source_user_message_ids {
        let Some(message) = user_by_id.get(source_id.as_str()) else {
            return Ok(false);
        };
        let has_request = match proposal.source {
            SpeakerNameProposalSource::UserRequest => {
                if proposal.name_edited_by_user {
                    user_message_requests_speaker_name(message)
                } else {
                    is_explicit_name_request(message, &proposal.name)
                }
            }
            SpeakerNameProposalSource::Inferred => {
                user_message_requests_speaker_name_inference(message)
            }
        };
        if !has_request {
            return Ok(false);
        }
    }

    let mut authorized_paths = BTreeSet::new();
    for value in &proposal.authorized_transcript_paths {
        let path = PathBuf::from(value);
        if !valid_indexed_path(&paths.transcripts, &path) || !authorized_paths.insert(path) {
            return Ok(false);
        }
    }
    let mut records = Vec::new();
    let mut records_by_path = std::collections::BTreeMap::new();
    for path in &authorized_paths {
        let path_records = JsonlStore::new(path.clone()).read::<TranscriptRecord>()?;
        records.extend(path_records.iter().cloned());
        records_by_path.insert(path.clone(), path_records);
    }
    let mut evidence_paths = Vec::with_capacity(proposal.evidence.len());
    for (evidence, path) in proposal
        .evidence
        .iter()
        .zip(&proposal.evidence_transcript_paths)
    {
        let evidence_path = PathBuf::from(path);
        if !authorized_paths.contains(&evidence_path) || !valid_evidence_shape(evidence) {
            return Ok(false);
        }
        evidence_paths.push(evidence_path);
    }
    match proposal.source {
        SpeakerNameProposalSource::UserRequest => {
            if proposal
                .evidence
                .iter()
                .any(|item| item.role.is_some() || item.group_id.is_some())
            {
                return Ok(false);
            }
            for (evidence, evidence_path) in proposal.evidence.iter().zip(&evidence_paths) {
                let Some(evidence_records) = records_by_path.get(evidence_path) else {
                    return Ok(false);
                };
                let matching_speakers = evidence_records
                    .iter()
                    .filter(|record| {
                        record.observation_id == evidence.observation_id
                            && record.audio_start_ms == Some(evidence.audio_start_ms)
                            && record.audio_end_ms == Some(evidence.audio_end_ms)
                            && record.source == "speaker"
                            && record.speaker_status
                                == Some(SpeakerIdentificationStatus::Identified)
                            && record.speaker_registry_id.as_deref() == Some(active_registry_id)
                            && record.text.contains(&evidence.quote)
                    })
                    .filter_map(|record| {
                        resolver
                            .resolve(
                                record.speaker_registry_id.as_deref()?,
                                record.speaker_tag.as_deref()?,
                            )
                            .map(|resolved| (resolved.canonical_id, resolved.prompt_id))
                    })
                    .collect::<Vec<_>>();
                let prompt_ids = matching_speakers
                    .iter()
                    .map(|(_, prompt_id)| prompt_id.clone())
                    .collect::<BTreeSet<_>>();
                if prompt_ids.len() != 1 || !prompt_ids.contains(&proposal.speaker_id) {
                    return Ok(false);
                }
                let ambiguous_quote = records
                    .iter()
                    .filter(|record| {
                        record.source == "speaker"
                            && record.speaker_status
                                == Some(SpeakerIdentificationStatus::Identified)
                            && record.speaker_registry_id.as_deref() == Some(active_registry_id)
                            && record.text.contains(&evidence.quote)
                            && record.audio_start_ms.is_some()
                            && record.audio_end_ms.is_some()
                    })
                    .filter_map(|record| {
                        resolver
                            .resolve(
                                record.speaker_registry_id.as_deref()?,
                                record.speaker_tag.as_deref()?,
                            )
                            .map(|resolved| resolved.prompt_id)
                    })
                    .collect::<BTreeSet<_>>();
                if ambiguous_quote.len() > 1 {
                    return Ok(false);
                }
            }
        }
        SpeakerNameProposalSource::Inferred => {
            if proposal.current_name.is_some()
                || !inferred_evidence_role_shape(&proposal.evidence)
                || verify_inferred_evidence(
                    &proposal.name,
                    &proposal.evidence,
                    &evidence_paths,
                    &records,
                    &records_by_path,
                    resolver,
                    active_registry_id,
                )
                .as_deref()
                    != Some(proposal.speaker_id.as_str())
            {
                return Ok(false);
            }
            let registered_name =
                crate::speaker_names::load_speaker_names(paths, active_registry_id)?
                    .get(&proposal.speaker_id)
                    .cloned();
            let recovering_own_apply = proposal.status == SpeakerNameProposalStatus::Applying
                && proposal.operation.as_ref().is_some_and(|operation| {
                    operation.before_name.is_none()
                        && operation.after_name == proposal.name
                        && registered_name.as_deref() == Some(proposal.name.as_str())
                });
            if registered_name.is_some() && !recovering_own_apply {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
