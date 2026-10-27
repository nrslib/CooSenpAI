use super::WorkProposal;
use crate::config::ConfigPaths;
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkOfferStatus {
    Proposed,
    Accepted,
    Declined,
    Ignored,
    Invalidated,
}

impl WorkOfferStatus {
    fn is_terminal(self) -> bool {
        self != Self::Proposed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkOffer {
    pub id: String,
    pub conversation_generation: u64,
    pub user_epoch: u64,
    pub observation_ids: Vec<String>,
    pub turn_id: String,
    pub entry_id: String,
    pub proposal: WorkProposal,
    pub fingerprint: String,
    pub status: WorkOfferStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_input_id: Option<String>,
    #[serde(default)]
    pub acceptance_enqueue_pending: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkOfferState {
    #[serde(default)]
    offers: Vec<WorkOffer>,
    #[serde(default)]
    user_epochs: BTreeMap<u64, u64>,
}

#[derive(Debug, Clone)]
pub struct WorkOfferStore {
    paths: ConfigPaths,
    path: PathBuf,
}

impl WorkOfferStore {
    pub fn new(paths: &ConfigPaths) -> Self {
        Self {
            paths: paths.clone(),
            path: paths.work_offers.clone(),
        }
    }

    pub fn propose(
        &self,
        generation: u64,
        expected_user_epoch: u64,
        turn_id: &str,
        entry_id: &str,
        observation_ids: Vec<String>,
        proposal: WorkProposal,
    ) -> Result<Option<WorkOffer>, PersistenceError> {
        self.update(|state| {
            let user_epoch = state.user_epochs.get(&generation).copied().unwrap_or(0);
            if user_epoch != expected_user_epoch {
                return None;
            }
            if let Some(existing) = state.offers.iter().find(|offer| offer.turn_id == turn_id) {
                return Some(existing.clone());
            }
            let fingerprint = fingerprint(&proposal);
            let suppressed = state.offers.iter().any(|offer| {
                offer.conversation_generation == generation
                    && (offer.status == WorkOfferStatus::Proposed
                        || offer.user_epoch == user_epoch
                        || (offer.status.is_terminal() && offer.fingerprint == fingerprint))
            });
            if suppressed {
                return None;
            }
            let offer = WorkOffer {
                id: format!("work-offer-{turn_id}"),
                conversation_generation: generation,
                user_epoch,
                observation_ids,
                turn_id: turn_id.to_owned(),
                entry_id: entry_id.to_owned(),
                proposal,
                fingerprint,
                status: WorkOfferStatus::Proposed,
                accepted_input_id: None,
                acceptance_enqueue_pending: false,
            };
            state.offers.push(offer.clone());
            Some(offer)
        })
    }

    pub(crate) fn accept(&self, id: &str, generation: u64) -> Result<WorkOffer, PersistenceError> {
        self.ensure_current_generation(generation)?;
        self.update(|state| {
            let offer = state
                .offers
                .iter_mut()
                .find(|offer| offer.id == id)
                .ok_or_else(|| PersistenceError::Invalid("作業の申し出が見つかりません".into()))?;
            if offer.conversation_generation != generation
                || offer.status != WorkOfferStatus::Proposed
            {
                return Err(PersistenceError::Invalid(
                    "この作業の申し出はすでに応答済みか、無効です".into(),
                ));
            }
            offer.status = WorkOfferStatus::Accepted;
            offer.accepted_input_id = Some(accepted_input_id(id));
            offer.acceptance_enqueue_pending = true;
            Ok(offer.clone())
        })?
    }

    pub(crate) fn decline(&self, id: &str, generation: u64) -> Result<WorkOffer, PersistenceError> {
        self.ensure_current_generation(generation)?;
        self.update(|state| {
            let offer = state
                .offers
                .iter_mut()
                .find(|offer| offer.id == id)
                .ok_or_else(|| PersistenceError::Invalid("作業の申し出が見つかりません".into()))?;
            if offer.conversation_generation != generation
                || offer.status != WorkOfferStatus::Proposed
            {
                return Err(PersistenceError::Invalid(
                    "この作業の申し出はすでに応答済みか、無効です".into(),
                ));
            }
            offer.status = WorkOfferStatus::Declined;
            Ok(offer.clone())
        })?
    }

    fn ensure_current_generation(&self, generation: u64) -> Result<(), PersistenceError> {
        if crate::conversation_archive::current_conversation_generation(&self.paths)? != generation
        {
            return Err(PersistenceError::Invalid(
                "会話が切り替わったため、この作業の申し出へは応答できません".into(),
            ));
        }
        Ok(())
    }

    pub fn complete_acceptance_enqueue(
        &self,
        id: &str,
        input_id: &str,
    ) -> Result<(), PersistenceError> {
        self.update(|state| {
            let offer = state
                .offers
                .iter_mut()
                .find(|offer| offer.id == id)
                .ok_or_else(|| PersistenceError::Invalid("作業の申し出が見つかりません".into()))?;
            if offer.status != WorkOfferStatus::Accepted
                || offer.accepted_input_id.as_deref() != Some(input_id)
            {
                return Err(PersistenceError::Invalid(
                    "受諾した作業の申し出とユーザー入力が一致しません".into(),
                ));
            }
            offer.acceptance_enqueue_pending = false;
            Ok(())
        })?
    }

    pub fn accepted_pending_enqueue(
        &self,
        generation: u64,
    ) -> Result<Vec<WorkOffer>, PersistenceError> {
        let _lock = self.lock()?;
        Ok(self
            .load_unlocked()?
            .offers
            .into_iter()
            .filter(|offer| {
                offer.conversation_generation == generation
                    && offer.status == WorkOfferStatus::Accepted
                    && offer.acceptance_enqueue_pending
            })
            .collect())
    }

    pub fn accepted_for_input(
        &self,
        input_id: &str,
    ) -> Result<Option<WorkOffer>, PersistenceError> {
        let _lock = self.lock()?;
        Ok(self.load_unlocked()?.offers.into_iter().find(|offer| {
            offer.status == WorkOfferStatus::Accepted
                && offer.accepted_input_id.as_deref() == Some(input_id)
        }))
    }

    pub fn pending(&self, generation: u64) -> Result<Option<WorkOffer>, PersistenceError> {
        let _lock = self.lock()?;
        Ok(self.load_unlocked()?.offers.into_iter().find(|offer| {
            offer.conversation_generation == generation && offer.status == WorkOfferStatus::Proposed
        }))
    }

    pub fn get(&self, id: &str) -> Result<Option<WorkOffer>, PersistenceError> {
        let _lock = self.lock()?;
        Ok(self
            .load_unlocked()?
            .offers
            .into_iter()
            .find(|offer| offer.id == id))
    }

    pub fn note_user_input(&self, generation: u64) -> Result<u64, PersistenceError> {
        self.update(|state| {
            let epoch = state
                .user_epochs
                .get(&generation)
                .copied()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| {
                    PersistenceError::Invalid("work offer user epoch overflow".to_owned())
                })?;
            state.user_epochs.insert(generation, epoch);
            for offer in state.offers.iter_mut().filter(|offer| {
                offer.conversation_generation == generation
                    && offer.status == WorkOfferStatus::Proposed
            }) {
                offer.status = WorkOfferStatus::Ignored;
            }
            Ok(epoch)
        })?
    }

    pub fn reconcile_user_epoch(
        &self,
        generation: u64,
        epoch: u64,
    ) -> Result<bool, PersistenceError> {
        self.update(|state| {
            let current = state.user_epochs.get(&generation).copied().unwrap_or(0);
            if current >= epoch {
                return Ok(false);
            }
            state.user_epochs.insert(generation, epoch);
            for offer in state.offers.iter_mut().filter(|offer| {
                offer.conversation_generation == generation
                    && offer.status == WorkOfferStatus::Proposed
            }) {
                offer.status = WorkOfferStatus::Ignored;
            }
            Ok(true)
        })?
    }

    pub fn invalidate_current(&self, generation: u64) -> Result<bool, PersistenceError> {
        self.update(|state| {
            let Some(offer) = state.offers.iter_mut().find(|offer| {
                offer.conversation_generation == generation
                    && offer.status == WorkOfferStatus::Proposed
            }) else {
                return false;
            };
            offer.status = WorkOfferStatus::Invalidated;
            true
        })
    }

    pub fn invalidate_all(&self) -> Result<usize, PersistenceError> {
        self.update(|state| {
            let mut invalidated = 0;
            for offer in state
                .offers
                .iter_mut()
                .filter(|offer| offer.status == WorkOfferStatus::Proposed)
            {
                offer.status = WorkOfferStatus::Invalidated;
                invalidated += 1;
            }
            invalidated
        })
    }

    fn update<T>(
        &self,
        update: impl FnOnce(&mut WorkOfferState) -> T,
    ) -> Result<T, PersistenceError> {
        let _lock = self.lock()?;
        let mut state = self.load_unlocked()?;
        let result = update(&mut state);
        atomic_write_json(&self.path, &state)?;
        Ok(result)
    }

    fn lock(&self) -> Result<SiblingLock, PersistenceError> {
        SiblingLock::acquire(&self.path.with_extension("json.lock"))
    }

    fn load_unlocked(&self) -> Result<WorkOfferState, PersistenceError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(WorkOfferState::default()),
            Err(error) => Err(error.into()),
        }
    }
}

pub(crate) fn accepted_input_id(offer_id: &str) -> String {
    let digest = Sha256::digest(format!("work-offer-acceptance-v1\0{offer_id}"));
    format!("runtime-user-work-offer-{:x}", digest)
}

pub(crate) fn accepted_message(proposal: &WorkProposal) -> String {
    let kind = match proposal.kind {
        super::WorkKind::Investigate => "investigate",
        super::WorkKind::Work => "work",
    };
    format!(
        "ユーザーはCooの申し出を受諾しました。これは作業の実行承認ではありません。\nkind: {kind}\ncwd: {}\nsummary: {}\n最初のworkRequestには、上記のkind、cwd、summaryをそのまま使ってください。",
        proposal.cwd, proposal.summary
    )
}

fn fingerprint(proposal: &WorkProposal) -> String {
    let cwd = normalize_cwd(&proposal.cwd);
    let summary = proposal
        .summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut digest = Sha256::new();
    digest.update(b"work-offer-v1\0");
    digest.update([match proposal.kind {
        super::WorkKind::Investigate => 0,
        super::WorkKind::Work => 1,
    }]);
    for value in [cwd.as_bytes(), summary.as_bytes()] {
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    format!("{:x}", digest.finalize())
}

fn normalize_cwd(cwd: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(cwd.trim()).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized.to_string_lossy().into_owned()
}
