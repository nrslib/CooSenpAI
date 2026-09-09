use crate::tutorial_events::{TutorialEvent, TutorialTask};
use crate::tutorial_notice::TutorialBubbleOutcome;
use crate::tutorial_progress_presenter::{step_intro_key, ProgressEvent};
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::runtime::RuntimeError;
use tokio::sync::oneshot;
type Reply = oneshot::Sender<Result<(), RuntimeError>>;
#[derive(Debug)]
pub(crate) enum LifecycleEvent {
    Started(Reply),
    Resumed {
        current: Option<TutorialStep>,
        watch_sequence: bool,
        follows_setup_ok: bool,
        reply: Reply,
    },
    GuidePresented {
        step: TutorialStep,
        result: Result<TutorialBubbleOutcome, RuntimeError>,
        reply: Reply,
    },
    MainOpened {
        should_emit: bool,
        reply: oneshot::Sender<()>,
    },
    MainGuidePresented {
        result: Result<TutorialBubbleOutcome, RuntimeError>,
        reply: oneshot::Sender<()>,
    },
    MainGuideRecorded(oneshot::Sender<()>),
    Finished {
        display: Result<TutorialBubbleOutcome, RuntimeError>,
        cleanup_ok: bool,
    },
    CleanupCompleted(oneshot::Sender<()>),
    CompletionShown(oneshot::Sender<()>),
    Done {
        result: Result<(), RuntimeError>,
        reply: Reply,
    },
}
#[derive(Debug)]
pub(crate) enum LifecycleTask {
    Intro {
        follows_setup_ok: bool,
        reopen: bool,
        reply: Reply,
    },
    ResumeSettings(Reply),
    ResumeGuide {
        step: TutorialStep,
        key: &'static str,
        reply: Reply,
    },
    MainGuide(oneshot::Sender<()>),
    RecordMainGuide {
        accepted: bool,
        reply: oneshot::Sender<()>,
    },
    ShowComplete(oneshot::Sender<()>),
}
pub(crate) fn handle(event: LifecycleEvent) -> Vec<UiEffect> {
    match event {
        LifecycleEvent::Started(reply) => vec![
            UiEffect::Deliver {
                child: PresenterId::Chat,
                event: UiEvent::Close,
            },
            task(LifecycleTask::Intro {
                follows_setup_ok: false,
                reopen: false,
                reply,
            }),
        ],
        LifecycleEvent::Resumed {
            current,
            watch_sequence,
            follows_setup_ok,
            reply,
        } => {
            if watch_sequence {
                return vec![task(LifecycleTask::ResumeSettings(reply))];
            }
            if let Some((step, key)) =
                current.and_then(|step| step_intro_key(step).map(|key| (step, key)))
            {
                vec![task(LifecycleTask::ResumeGuide { step, key, reply })]
            } else if current == Some(TutorialStep::Chat) {
                vec![task(LifecycleTask::Intro {
                    follows_setup_ok,
                    reopen: true,
                    reply,
                })]
            } else {
                let _ = reply.send(Ok(()));
                Vec::new()
            }
        }
        LifecycleEvent::GuidePresented {
            step,
            result,
            reply,
        } => {
            let advance = matches!(result, Ok(TutorialBubbleOutcome::Acknowledged));
            let _ = reply.send(result.map(|_| ()));
            if advance {
                vec![UiEffect::Deliver {
                    child: PresenterId::Tutorial,
                    event: UiEvent::Tutorial(Box::new(TutorialEvent::Progress(
                        ProgressEvent::Guide(step),
                    ))),
                }]
            } else {
                Vec::new()
            }
        }
        LifecycleEvent::MainOpened { should_emit, reply } => {
            if should_emit {
                vec![task(LifecycleTask::MainGuide(reply))]
            } else {
                let _ = reply.send(());
                Vec::new()
            }
        }
        LifecycleEvent::MainGuidePresented { result, reply } => {
            vec![task(LifecycleTask::RecordMainGuide {
                accepted: matches!(result, Ok(TutorialBubbleOutcome::Acknowledged)),
                reply,
            })]
        }
        LifecycleEvent::MainGuideRecorded(reply) => {
            let _ = reply.send(());
            Vec::new()
        }
        LifecycleEvent::Finished {
            display,
            cleanup_ok,
        } => {
            if finish_display_is_accepted(&display) && cleanup_ok {
                vec![UiEffect::Deliver {
                    child: PresenterId::Chat,
                    event: UiEvent::OpenMain,
                }]
            } else {
                Vec::new()
            }
        }
        LifecycleEvent::CleanupCompleted(reply) => vec![task(LifecycleTask::ShowComplete(reply))],
        LifecycleEvent::CompletionShown(reply) => {
            let _ = reply.send(());
            Vec::new()
        }
        LifecycleEvent::Done { result, reply } => {
            let _ = reply.send(result);
            Vec::new()
        }
    }
}
pub(crate) fn event(value: LifecycleEvent) -> UiEvent {
    UiEvent::Tutorial(Box::new(TutorialEvent::Lifecycle(value)))
}
fn task(value: LifecycleTask) -> UiEffect {
    UiEffect::Spawn(UiTask::Tutorial(TutorialTask::Lifecycle(value)))
}
pub(crate) fn finish_display_is_accepted(
    outcome: &Result<TutorialBubbleOutcome, RuntimeError>,
) -> bool {
    matches!(outcome, Ok(TutorialBubbleOutcome::Acknowledged))
}

pub(crate) async fn main_opened(
    tutorial: &tokio::sync::Mutex<crate::tutorial::TutorialController>,
    ui: &crate::ui_root::UiHandle,
) {
    let should_emit = tutorial.lock().await.take_chat_opened();
    let _ = ui
        .query(crate::ui_events::UiView::Application, |reply| {
            event(LifecycleEvent::MainOpened { should_emit, reply })
        })
        .await;
}
