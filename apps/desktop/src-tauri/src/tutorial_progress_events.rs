use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::runtime::RuntimeError;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub(crate) type Reply = oneshot::Sender<Result<(), RuntimeError>>;
#[derive(Debug)]
pub(crate) enum ProgressEvent {
    Settings(Reply),
    Watch(Reply),
    WatchDelayExpired(u64),
    Finish {
        step: TutorialStep,
        skipped: bool,
        automatic: bool,
        reply: Reply,
    },
    Response {
        entry_id: String,
        message: String,
        reply: Reply,
    },
    Guide(TutorialStep),
    Completed {
        id: u64,
        result: ProgressResult,
    },
    TransitionExpired {
        id: u64,
        cancellation: CancellationToken,
    },
    AdvanceChecked {
        step: TutorialStep,
        current: Option<TutorialStep>,
        presented: bool,
        guide: bool,
        reply: oneshot::Sender<bool>,
    },
}
#[derive(Debug)]
pub(crate) enum ProgressResult {
    Settings(Option<crate::tutorial::TutorialSettingsHighlight>),
    Watch(bool),
    Current(Option<TutorialStep>),
    Step(Result<Option<TutorialStep>, RuntimeError>),
    Response(Option<TutorialStep>),
    Guide(Result<(String, String), String>),
    Cleared {
        cleared: bool,
        cancellation: CancellationToken,
    },
    Read(bool),
    Advanced(Result<bool, RuntimeError>),
    Permission,
    Presented(Result<crate::tutorial_notice::TutorialBubbleOutcome, RuntimeError>),
    Done(Result<(), RuntimeError>),
}
#[derive(Debug)]
pub(crate) struct ProgressTask {
    pub id: u64,
    pub action: ProgressAction,
}
#[derive(Debug)]
pub(crate) enum ProgressAction {
    BeginSettings,
    CompleteSettings,
    FailSettings,
    WatchSequence,
    BeginWatch,
    PresentAfterWatch,
    FailWatch,
    Current,
    FinishWatch,
    Finish {
        step: TutorialStep,
        skipped: bool,
    },
    AcceptResponse {
        entry_id: String,
        message: String,
    },
    GuideDetails(&'static str),
    Clear,
    Read {
        notice_id: String,
        delay: std::time::Duration,
    },
    Permission(TutorialStep),
    Present(&'static str),
    Advance {
        step: TutorialStep,
        notice_id: String,
        guide: bool,
    },
    FinishTutorial,
}
