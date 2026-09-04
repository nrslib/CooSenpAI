use crate::onboarding::{OnboardingError, OnboardingState};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TutorialNoticeState {
    pub created_at: String,
    pub message: String,
    #[serde(default)]
    pub conversation_stored: bool,
    #[serde(default)]
    pub bubble_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TutorialNoticePlan {
    pub id: String,
    pub key: String,
    pub created_at: String,
    pub message: String,
    pub conversation_stored: bool,
    pub bubble_accepted: bool,
}

impl OnboardingState {
    pub fn prepare_tutorial_notice_sequence(
        &mut self,
        notices: &[(&str, &str)],
        created_at: &str,
    ) -> Result<(), OnboardingError> {
        let base = DateTime::parse_from_rfc3339(created_at)
            .map_err(|error| {
                OnboardingError::Invalid(format!("tutorial 案内の日時が不正です: {error}"))
            })?
            .with_timezone(&Utc);
        for (offset, (key, message)) in notices.iter().enumerate() {
            let created_at = (base + Duration::milliseconds(offset as i64))
                .to_rfc3339_opts(SecondsFormat::Millis, true);
            self.prepare_tutorial_notice(key, message, &created_at)?;
        }
        Ok(())
    }

    pub fn pending_tutorial_notices(&self) -> Result<Vec<TutorialNoticePlan>, OnboardingError> {
        let mut plans = self
            .tutorial
            .notices
            .iter()
            .filter(|(_, notice)| !notice.conversation_stored || !notice.bubble_accepted)
            .map(|(key, notice)| {
                Ok(TutorialNoticePlan {
                    id: self.tutorial_notice_id(key)?,
                    key: key.clone(),
                    created_at: notice.created_at.clone(),
                    message: notice.message.clone(),
                    conversation_stored: notice.conversation_stored,
                    bubble_accepted: notice.bubble_accepted,
                })
            })
            .collect::<Result<Vec<_>, OnboardingError>>()?;
        plans.sort_by(|left, right| {
            (&left.created_at, &left.key).cmp(&(&right.created_at, &right.key))
        });
        Ok(plans)
    }

    pub fn tutorial_notice_id(&self, key: &str) -> Result<String, OnboardingError> {
        let run_id =
            self.tutorial.run_id.as_deref().ok_or_else(|| {
                OnboardingError::Invalid("tutorial run ID がありません".to_owned())
            })?;
        Ok(format!("tutorial-{run_id}-{key}"))
    }

    pub fn prepare_tutorial_notice(
        &mut self,
        key: &str,
        message: &str,
        created_at: &str,
    ) -> Result<TutorialNoticePlan, OnboardingError> {
        let id = self.tutorial_notice_id(key)?;
        let state = self
            .tutorial
            .notices
            .entry(key.to_owned())
            .or_insert_with(|| TutorialNoticeState {
                created_at: created_at.to_owned(),
                message: message.to_owned(),
                conversation_stored: false,
                bubble_accepted: false,
            });
        Ok(TutorialNoticePlan {
            id,
            key: key.to_owned(),
            created_at: state.created_at.clone(),
            message: state.message.clone(),
            conversation_stored: state.conversation_stored,
            bubble_accepted: state.bubble_accepted,
        })
    }

    pub fn mark_tutorial_notice_conversation_stored(
        &mut self,
        key: &str,
    ) -> Result<(), OnboardingError> {
        self.tutorial_notice_mut(key)?.conversation_stored = true;
        Ok(())
    }

    pub fn mark_tutorial_notice_bubble_accepted(
        &mut self,
        key: &str,
    ) -> Result<(), OnboardingError> {
        self.tutorial_notice_mut(key)?.bubble_accepted = true;
        Ok(())
    }

    pub fn reopen_tutorial_notice_bubble(&mut self, key: &str) -> Result<(), OnboardingError> {
        self.tutorial_notice_mut(key)?.bubble_accepted = false;
        Ok(())
    }

    fn tutorial_notice_mut(
        &mut self,
        key: &str,
    ) -> Result<&mut TutorialNoticeState, OnboardingError> {
        self.tutorial.notices.get_mut(key).ok_or_else(|| {
            OnboardingError::Invalid(format!("tutorial 案内が prepare されていません: {key}"))
        })
    }
}
