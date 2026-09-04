use crate::tutorial::TutorialController;
use async_trait::async_trait;
use coosenpai_core::onboarding_notice::TutorialNoticePlan;
use coosenpai_core::runtime::RuntimeError;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TutorialBubbleOutcome {
    Acknowledged,
    Dismissed,
}

#[async_trait]
pub(crate) trait TutorialNoticeEffects: Send + Sync {
    async fn append_conversation(&self, notice: &TutorialNoticePlan) -> Result<(), RuntimeError>;

    async fn present_bubble(
        &self,
        notice: &TutorialNoticePlan,
    ) -> Result<TutorialBubbleOutcome, RuntimeError>;
}

pub(crate) async fn deliver(
    tutorial: &Mutex<TutorialController>,
    key: &str,
    message: &str,
    effects: &dyn TutorialNoticeEffects,
) -> Result<TutorialBubbleOutcome, RuntimeError> {
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut plan = tutorial
        .lock()
        .await
        .prepare_notice(key, message, &created_at)
        .map_err(onboarding_error)?;
    deliver_plan(tutorial, &mut plan, effects).await
}

#[cfg(test)]
pub(crate) async fn reconcile(
    tutorial: &Mutex<TutorialController>,
    effects: &dyn TutorialNoticeEffects,
) -> Result<TutorialBubbleOutcome, RuntimeError> {
    reconcile_except(tutorial, effects, &[]).await
}

pub(crate) async fn reconcile_except(
    tutorial: &Mutex<TutorialController>,
    effects: &dyn TutorialNoticeEffects,
    excluded_keys: &[&str],
) -> Result<TutorialBubbleOutcome, RuntimeError> {
    let plans = tutorial
        .lock()
        .await
        .state()
        .pending_tutorial_notices()
        .map_err(onboarding_error)?;
    for mut plan in plans
        .into_iter()
        .filter(|plan| !excluded_keys.contains(&plan.key.as_str()))
    {
        let outcome = deliver_plan(tutorial, &mut plan, effects).await?;
        if outcome == TutorialBubbleOutcome::Dismissed {
            return Ok(outcome);
        }
    }
    Ok(TutorialBubbleOutcome::Acknowledged)
}

async fn deliver_plan(
    tutorial: &Mutex<TutorialController>,
    plan: &mut TutorialNoticePlan,
    effects: &dyn TutorialNoticeEffects,
) -> Result<TutorialBubbleOutcome, RuntimeError> {
    if !plan.conversation_stored {
        effects.append_conversation(plan).await?;
        tutorial
            .lock()
            .await
            .mark_notice_conversation_stored(&plan.key)
            .map_err(onboarding_error)?;
        plan.conversation_stored = true;
    }
    if plan.bubble_accepted {
        return Ok(TutorialBubbleOutcome::Acknowledged);
    }
    let outcome = effects.present_bubble(plan).await?;
    if outcome == TutorialBubbleOutcome::Acknowledged && !plan.bubble_accepted {
        tutorial
            .lock()
            .await
            .mark_notice_bubble_accepted(&plan.key)
            .map_err(onboarding_error)?;
        plan.bubble_accepted = true;
    }
    Ok(outcome)
}

fn onboarding_error(error: coosenpai_core::onboarding::OnboardingError) -> RuntimeError {
    RuntimeError::Factory(error.to_string())
}

