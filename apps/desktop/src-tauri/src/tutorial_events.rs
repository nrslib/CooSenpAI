use crate::bubbles::BubbleMilestone;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) enum CardEvent {
    WaitCard {
        notice_id: String,
        milestone: BubbleMilestone,
        main_delay: std::time::Duration,
        cancellation: CancellationToken,
        reply: oneshot::Sender<bool>,
    },
    CardLoaded {
        id: u64,
        signals: Option<(CancellationToken, CancellationToken)>,
    },
    CardObserved {
        id: u64,
        signal: CardSignal,
    },
    CardDelayExpired(u64),
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum CardSignal {
    Reached,
    Dismissed,
    Cancelled,
}

#[derive(Debug)]
pub(crate) enum CardTask {
    ReadCard {
        id: u64,
        notice_id: String,
        milestone: BubbleMilestone,
    },
    ObserveCard {
        id: u64,
        reached: CancellationToken,
        dismissed: CancellationToken,
        cancellation: CancellationToken,
    },
    ObserveCancellation {
        id: u64,
        cancellation: CancellationToken,
    },
}

#[derive(Debug)]
pub(crate) enum TutorialEvent {
    Lifecycle(crate::tutorial_lifecycle_presenter::LifecycleEvent),
    Response(crate::tutorial_response_presenter::ResponseEvent),
    Progress(crate::tutorial_progress_presenter::ProgressEvent),
    Notice(crate::tutorial_notice_presenter::NoticeEvent),
    Card(CardEvent),
    Sequence(crate::tutorial_sequence_presenter::SequenceEvent),
}

#[derive(Debug)]
pub(crate) enum TutorialTask {
    Lifecycle(crate::tutorial_lifecycle_presenter::LifecycleTask),
    Response(crate::tutorial_response_presenter::ResponseTask),
    Progress(crate::tutorial_progress_presenter::ProgressTask),
    Notice(crate::tutorial_notice_presenter::NoticeTask),
    Card(CardTask),
    Sequence(crate::tutorial_sequence_presenter::SequenceTask),
}

impl TutorialEvent {
    pub(crate) fn label(&self) -> String {
        use crate::tutorial_lifecycle_presenter::LifecycleEvent;
        use crate::tutorial_notice_presenter::NoticeEvent;
        use crate::tutorial_progress_presenter::ProgressEvent;
        use crate::tutorial_response_presenter::ResponseEvent;
        use crate::tutorial_sequence_presenter::SequenceEvent;
        match self {
            Self::Lifecycle(event) => match event {
                LifecycleEvent::Started(..) => "Tutorial(Lifecycle.Started)".into(),
                LifecycleEvent::Resumed { .. } => "Tutorial(Lifecycle.Resumed)".into(),
                LifecycleEvent::GuidePresented { .. } => {
                    "Tutorial(Lifecycle.GuidePresented)".into()
                }
                LifecycleEvent::MainOpened { .. } => "Tutorial(Lifecycle.MainOpened)".into(),
                LifecycleEvent::MainGuidePresented { .. } => {
                    "Tutorial(Lifecycle.MainGuidePresented)".into()
                }
                LifecycleEvent::MainGuideRecorded(..) => {
                    "Tutorial(Lifecycle.MainGuideRecorded)".into()
                }
                LifecycleEvent::Finished { .. } => "Tutorial(Lifecycle.Finished)".into(),
                LifecycleEvent::CleanupCompleted(..) => {
                    "Tutorial(Lifecycle.CleanupCompleted)".into()
                }
                LifecycleEvent::CompletionShown(..) => "Tutorial(Lifecycle.CompletionShown)".into(),
                LifecycleEvent::Done { .. } => "Tutorial(Lifecycle.Done)".into(),
            },
            Self::Response(event) => match event {
                ResponseEvent::Finished => "Tutorial(Response.Finished)".into(),
                ResponseEvent::PendingLoaded { .. } => "Tutorial(Response.PendingLoaded)".into(),
                ResponseEvent::Prepared { .. } => "Tutorial(Response.Prepared)".into(),
                ResponseEvent::Presented { .. } => "Tutorial(Response.Presented)".into(),
                ResponseEvent::Done(..) => "Tutorial(Response.Done)".into(),
                ResponseEvent::NotificationCompleted { .. } => {
                    "Tutorial(Response.NotificationCompleted)".into()
                }
                ResponseEvent::RuntimeConversationLoaded => {
                    "Tutorial(Response.RuntimeConversationLoaded)".into()
                }
                ResponseEvent::NotificationRefreshed { .. } => {
                    "Tutorial(Response.NotificationRefreshed)".into()
                }
                ResponseEvent::NotificationRecorded(..) => {
                    "Tutorial(Response.NotificationRecorded)".into()
                }
            },
            Self::Progress(event) => match event {
                ProgressEvent::Settings(..) => "Tutorial(Progress.Settings)".into(),
                ProgressEvent::Watch(..) => "Tutorial(Progress.Watch)".into(),
                ProgressEvent::WatchDelayExpired(..) => {
                    "Tutorial(Progress.WatchDelayExpired)".into()
                }
                ProgressEvent::Finish { .. } => "Tutorial(Progress.Finish)".into(),
                ProgressEvent::Response { .. } => "Tutorial(Progress.Response)".into(),
                ProgressEvent::Guide(..) => "Tutorial(Progress.Guide)".into(),
                ProgressEvent::Completed { .. } => "Tutorial(Progress.Completed)".into(),
                ProgressEvent::TransitionExpired { .. } => {
                    "Tutorial(Progress.TransitionExpired)".into()
                }
                ProgressEvent::AdvanceChecked { .. } => "Tutorial(Progress.AdvanceChecked)".into(),
            },
            Self::Notice(event) => match event {
                NoticeEvent::Requested { .. } => "Tutorial(Notice.Requested)".into(),
                NoticeEvent::Presented { .. } => "Tutorial(Notice.Presented)".into(),
                NoticeEvent::ConversationSaved => "Tutorial(Notice.ConversationSaved)".into(),
            },
            Self::Card(event) => match event {
                CardEvent::WaitCard { .. } => "Tutorial(Card.WaitCard)".into(),
                CardEvent::CardLoaded { .. } => "Tutorial(Card.CardLoaded)".into(),
                CardEvent::CardObserved { .. } => "Tutorial(Card.CardObserved)".into(),
                CardEvent::CardDelayExpired(..) => "Tutorial(Card.CardDelayExpired)".into(),
            },
            Self::Sequence(event) => match event {
                SequenceEvent::Start { .. } => "Tutorial(Sequence.Start)".into(),
                SequenceEvent::Cancel(..) => "Tutorial(Sequence.Cancel)".into(),
                SequenceEvent::Presented { .. } => "Tutorial(Sequence.Presented)".into(),
                SequenceEvent::Reached { .. } => "Tutorial(Sequence.Reached)".into(),
            },
        }
    }
}
