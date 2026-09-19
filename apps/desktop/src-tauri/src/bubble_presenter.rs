#[path = "bubble_notification.rs"]
mod notification;
#[path = "bubble_presentation.rs"]
mod presentation_state;
pub(crate) use notification::NotificationContext;
use notification::NotificationPlan;

use super::{BubblePresentation, BubbleRecord, BubbleState};
use crate::presentation::PresentationEvent;
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, UiTask, ViewCommand};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct BubblePresenter {
    pub(crate) tutorial: Option<Arc<Mutex<crate::tutorial::TutorialController>>>,
    controls: crate::bubble_controls_presenter::BubbleControlsPresenter,
    expiry_epoch: u64,
    expiry_deadline: Option<Instant>,
    thought: crate::thought_presenter::ThoughtPresenter,
    model: Arc<Mutex<BubbleState>>,
    config: coosenpai_core::config::Config,
    avatar_image_png: Option<Vec<u8>>,
    delivery_log: notification::BubbleDeliveryLogState,
    presentation: crate::presentation::Presentation,
    snapshot: Option<std::sync::Arc<crate::bubbles::BubbleSnapshot>>,
    display: String,
    typing_epoch: u64,
    typing: Option<(String, usize, usize)>,
    edge_recall: crate::bubble_edge_recall::EdgeRecallDebounce,
    onboarding_disables_edge_recall: bool,
    conversation_generation: u64,
    main_focused: bool,
}

#[derive(Debug)]
pub(crate) struct BubbleContent {
    pub snapshot: Arc<super::BubbleSnapshot>,
    pub display: String,
}

pub(crate) type BubbleWindowEvent = PresentationEvent<BubbleContent, BubbleContent>;

impl BubblePresenter {
    pub(crate) fn new(
        config: coosenpai_core::config::Config,
        conversation_generation: u64,
    ) -> Self {
        Self {
            model: Arc::new(Mutex::new(BubbleState::for_conversation_generation(
                conversation_generation,
            ))),
            config,
            ..Self::default()
        }
    }

    pub(crate) fn initialize(&mut self, snapshot: &crate::snapshot::AppSnapshot) {
        let onboarding_disables_edge_recall =
            snapshot.onboarding.setup_required || snapshot.onboarding.tutorial_active;
        let reset_edge_recall = self.config.revision != snapshot.config.revision
            || self.config.bubble.edge_recall != snapshot.config.bubble.edge_recall
            || self.config.bubble.position != snapshot.config.bubble.position
            || self.config.bubble.display != snapshot.config.bubble.display
            || self.conversation_generation != snapshot.selected_conversation_generation
            || (!self.onboarding_disables_edge_recall && onboarding_disables_edge_recall);
        self.config = snapshot.config.clone();
        self.avatar_image_png = snapshot.avatar_image_png.clone();
        self.onboarding_disables_edge_recall = onboarding_disables_edge_recall;
        self.conversation_generation = snapshot.selected_conversation_generation;
        if reset_edge_recall {
            self.reset_edge_recall();
        }
    }

    pub(crate) async fn sync_latest_coo_speech(&self, snapshot: &crate::snapshot::AppSnapshot) {
        let record = latest_coo_speech(snapshot);
        self.model.lock().await.set_latest_coo_speech(record);
    }

    fn reset_edge_recall(&mut self) {
        self.edge_recall = crate::bubble_edge_recall::EdgeRecallDebounce::default();
    }

    pub(crate) async fn handle_input(
        &mut self,
        event: UiEvent,
        main_focused: bool,
        main_visible: bool,
    ) -> Handling {
        if main_focused && !self.main_focused {
            self.reset_edge_recall();
        }
        self.main_focused = main_focused;
        match event {
            UiEvent::BubbleClick { id, body } => {
                let intro_click = if let Some(tutorial) = &self.tutorial {
                    tutorial
                        .lock()
                        .await
                        .state()
                        .tutorial_notice_id("intro-click")
                        .is_ok_and(|notice_id| notice_id == id)
                } else {
                    false
                };
                let mut model = self.model.lock().await;
                model.expire(Instant::now());
                if body
                    && !intro_click
                    && model.front_is_reading()
                    && model.front_record().is_some_and(|record| record.id == id)
                {
                    return Handling::Handled(vec![UiEffect::Run(UiTask::BubbleFastForward(id))]);
                }
                let Some(target) = model.click_target(&id) else {
                    return Handling::Handled(vec![UiEffect::Log(
                        "ui: presenter=Bubble input=Body ignored=true reason=card-unavailable"
                            .into(),
                    )]);
                };
                Handling::Handled(vec![UiEffect::Deliver {
                    child: PresenterId::Root,
                    event: UiEvent::Window {
                        view: PresenterId::Chat,
                        event: PresentationEvent::Open(crate::ui_load::WindowRequest::BubbleClick(
                            target,
                        )),
                    },
                }])
            }
            UiEvent::BubbleClickPrepared {
                generation,
                target,
                result,
            } => {
                let mut model = self.model.lock().await;
                model.expire(Instant::now());
                let event = if !model.accepts_click(&target) {
                    PresentationEvent::CancelLoad { generation }
                } else {
                    let result = result.and_then(|content| match content {
                        Some(crate::ui_load::WindowContent::Main(main)) => {
                            let mut main = Arc::unwrap_or_clone(main);
                            main.bubble_click = Some(target);
                            Ok(Some(crate::ui_load::WindowContent::Main(Arc::new(main))))
                        }
                        None => Ok(None),
                        Some(_) => {
                            Err("吹き出しクリックの読込結果がメイン画面ではありません".to_owned())
                        }
                    });
                    PresentationEvent::Loaded { generation, result }
                };
                Handling::Handled(vec![UiEffect::Deliver {
                    child: PresenterId::Root,
                    event: UiEvent::Window {
                        view: PresenterId::Chat,
                        event,
                    },
                }])
            }
            UiEvent::BubbleClickCompleted(target) => self.complete_click(target).await,

            UiEvent::BubbleView(event) => {
                use crate::bubble_controls_presenter::{BubbleControlAction, BubbleViewEvent};
                let mut ignored = None;
                let actions = match event {
                    BubbleViewEvent::Input(input) => {
                        let label = input.label();
                        let actions = self.controls.input(input, Instant::now());
                        if actions.is_empty() {
                            ignored = Some(UiEffect::Log(format!(
                                "ui: presenter=Bubble input={label} ignored=true reason={}",
                                self.controls.ignored_reason
                            )));
                        }
                        actions
                    }
                    BubbleViewEvent::Completed { token, id, result } => {
                        self.controls.complete(token, &id, result);
                        Vec::new()
                    }
                };
                let mut effects = vec![UiEffect::BubbleControls(Box::new(
                    self.controls.view.clone(),
                ))];
                effects.extend(ignored);
                for action in actions {
                    match action {
                        BubbleControlAction::Focus => effects.push(UiEffect::View {
                            view: PresenterId::Bubble,
                            command: ViewCommand::Front,
                        }),
                        BubbleControlAction::Body { id, open_main } => {
                            effects.push(UiEffect::Deliver {
                                child: PresenterId::Bubble,
                                event: UiEvent::BubbleClick {
                                    id,
                                    body: !open_main,
                                },
                            })
                        }
                        BubbleControlAction::Navigate(direction) => {
                            let (reply, _) = tokio::sync::oneshot::channel();
                            effects.push(UiEffect::Deliver {
                                child: PresenterId::Bubble,
                                event: UiEvent::UserCommand(
                                    crate::ui_commands::UserCommand::BubbleNavigate {
                                        direction,
                                        reply,
                                    },
                                ),
                            });
                        }
                        BubbleControlAction::Dismiss { id, token } => {
                            let restart_setup =
                                self.model.lock().await.restarts_setup_on_dismiss(&id);
                            effects.push(UiEffect::Spawn(UiTask::BubbleView {
                                id,
                                token,
                                action: None,
                                value: None,
                                restart_setup,
                            }));
                        }
                        BubbleControlAction::Interact {
                            id,
                            token,
                            action,
                            value,
                        } => effects.push(UiEffect::Spawn(UiTask::BubbleView {
                            id,
                            token,
                            action: Some(action),
                            value,
                            restart_setup: false,
                        })),
                    }
                }
                Handling::Handled(effects)
            }
            UiEvent::UserCommand(crate::ui_commands::UserCommand::BubbleDismiss { id, reply }) => {
                let (allows, restart_setup) = {
                    let model = self.model.lock().await;
                    (
                        model.allows_manual_dismiss(&id),
                        model.restarts_setup_on_dismiss(&id),
                    )
                };
                if !allows {
                    let _ = reply.send(crate::commands::IpcResult::failure(
                        coosenpai_core::locale::text(
                            coosenpai_core::locale::TextKey::TutorialManualDismissNotAllowed,
                            coosenpai_core::locale::Locale::from_config(&self.config.ui.language),
                        ),
                    ));
                    Handling::Handled(Vec::new())
                } else {
                    Handling::Handled(vec![UiEffect::Spawn(UiTask::DismissBubble {
                        id,
                        restart_setup,
                        reply,
                    })])
                }
            }
            UiEvent::UserCommand(command) if command.owner() == PresenterId::Bubble => {
                Handling::Handled(vec![UiEffect::Spawn(UiTask::UserCommand(command))])
            }
            UiEvent::CommandFinished {
                owner: PresenterId::Bubble,
                completion,
            } => {
                completion.reply();
                Handling::Handled(Vec::new())
            }
            UiEvent::BubbleAck { generation, reply } => {
                let (accepted, setup) = {
                    let model = self.model.lock().await;
                    (
                        model.acknowledge(generation),
                        model.record_for_message_kind("setup").is_some(),
                    )
                };
                let result = if accepted {
                    crate::commands::IpcResult::success(())
                } else {
                    crate::commands::IpcResult::failure(coosenpai_core::locale::text(
                        coosenpai_core::locale::TextKey::BubbleGenerationUnknown,
                        coosenpai_core::locale::Locale::from_config(&self.config.ui.language),
                    ))
                };
                let _ = reply.send(result);
                Handling::Handled(if accepted && setup {
                    vec![UiEffect::Log(format!(
                        "初回セットアップ吹き出しの表示ACKを受信しました: generation={generation}"
                    ))]
                } else {
                    Vec::new()
                })
            }
            UiEvent::BubbleExpiry(epoch) => {
                if epoch != self.expiry_epoch {
                    return Handling::Handled(Vec::new());
                }
                self.expiry_deadline = None;
                self.model.lock().await.expire(Instant::now());
                self.refresh().await
            }
            UiEvent::BubbleEdgePoll {
                at_edge,
                config_revision,
            } => {
                self.handle_edge_poll(at_edge, config_revision, main_focused)
                    .await
            }
            UiEvent::BubbleEdgeRecallReset => {
                self.reset_edge_recall();
                Handling::Handled(Vec::new())
            }
            UiEvent::BubbleSnapshot(reply) => {
                let (snapshot, _) = self.snapshot().await;
                let _ = reply.send(crate::commands::IpcResult::success((*snapshot).clone()));
                Handling::Handled(Vec::new())
            }
            UiEvent::BubbleQuery(query) => {
                use crate::ui_events::BubbleQuery;
                let model = self.model.lock().await;
                match query {
                    BubbleQuery::ConversationGeneration(reply) => {
                        let _ = reply.send(model.conversation_generation());
                    }
                    BubbleQuery::AcceptsInteraction {
                        id,
                        action,
                        value,
                        reply,
                    } => {
                        let _ =
                            reply.send(model.accepts_interaction(&id, &action, value.as_deref()));
                    }
                    BubbleQuery::CanPollEdgeRecall(reply) => {
                        let _ = reply.send(model.can_poll_edge_recall());
                    }
                    BubbleQuery::SetupRecord(reply) => {
                        let _ = reply.send(model.record_for_message_kind("setup"));
                    }
                    BubbleQuery::CardCompletion {
                        id,
                        milestone,
                        reply,
                    } => {
                        let _ = reply.send(model.card_completion(&id, milestone));
                    }
                }
                Handling::Handled(Vec::new())
            }

            UiEvent::BubbleMutation { mutation, reply } => {
                use super::BubbleMutation;
                let hover = matches!(mutation, BubbleMutation::Hover { .. });
                let appearance = matches!(mutation, BubbleMutation::Preview(_));
                let conversation_generation_mutation =
                    matches!(mutation, BubbleMutation::ConversationGeneration(_));
                let (changed, conversation_generation) = {
                    let mut model = self.model.lock().await;
                    let changed = match mutation {
                        BubbleMutation::ConversationGeneration(generation) => {
                            model.advance_conversation_generation(generation)
                        }
                        BubbleMutation::SwitchConversationGeneration(generation) => {
                            model.switch_conversation_generation(generation)
                        }
                        BubbleMutation::Preview(preview) => model.set_appearance_preview(preview),
                        BubbleMutation::FastForward(id) => {
                            model.fast_forward(id.as_deref(), Instant::now())
                        }
                        BubbleMutation::Navigate(direction) => {
                            model.navigate(direction, Instant::now())
                        }
                        BubbleMutation::Dismiss(id) => model.dismiss(&id),
                        BubbleMutation::CompleteAction(id) => model.complete_action(&id),
                        BubbleMutation::ClearTutorialProgress => model.clear_tutorial_progress(),
                        BubbleMutation::ClearThoughtBubbles => model.clear_thought_bubbles(),
                        BubbleMutation::SetMaxStack(max_stack) => model.set_max_stack(max_stack),
                        BubbleMutation::DismissMessageKind(kind) => {
                            model.dismiss_message_kind(&kind)
                        }
                        BubbleMutation::SetInteraction { id, interaction } => {
                            model.set_interaction(&id, interaction.map(|value| *value))
                        }
                        BubbleMutation::Hover { id, hovering } => {
                            model.set_hover(&id, hovering);
                            false
                        }
                        #[cfg(test)]
                        BubbleMutation::Seed {
                            record,
                            shown_ago,
                            duration,
                            replaced_ids,
                        } => model.show_replacing(
                            *record,
                            Instant::now() - shown_ago,
                            duration,
                            self.config.bubble.max_stack,
                            &replaced_ids,
                        ),
                    };
                    (changed, model.conversation_generation())
                };
                if conversation_generation_mutation && changed {
                    self.conversation_generation = conversation_generation;
                    self.reset_edge_recall();
                }
                let _ = reply.send(changed);
                if appearance {
                    self.refresh_appearance().await
                } else if hover {
                    Handling::Handled(self.schedule_expiry().await)
                } else if changed {
                    self.refresh().await
                } else {
                    Handling::Handled(Vec::new())
                }
            }

            UiEvent::NotificationRequested {
                record,
                target,
                context,
                reply,
            } => {
                let (plan, mut effects) = self
                    .notification(record, target, *context, main_focused, main_visible)
                    .await;
                match plan {
                    NotificationPlan::Complete(accepted) => {
                        let _ = reply.send(accepted);
                    }
                    NotificationPlan::Unread => effects.push(UiEffect::Deliver {
                        child: crate::ui_events::PresenterId::Root,
                        event: UiEvent::SnapshotCompleted(Box::new(
                            crate::snapshot_presenter::SnapshotEvent::UnreadAdded(reply),
                        )),
                    }),
                    NotificationPlan::Bubble(presentation) => {
                        effects.push(UiEffect::Run(UiTask::CompleteNotification {
                            presentation,
                            reply,
                        }))
                    }
                    NotificationPlan::Os(record) => {
                        effects.push(UiEffect::Run(UiTask::PrepareOsNotification {
                            record,
                            reply,
                        }))
                    }
                }
                Handling::Handled(effects)
            }
            UiEvent::OsNotificationPrepared {
                record,
                guard,
                reply,
            } => {
                let Some(guard) = guard else {
                    let _ = reply.send(false);
                    return Handling::Handled(Vec::new());
                };
                if record.conversation_generation
                    != self.model.lock().await.conversation_generation()
                {
                    let _ = reply.send(false);
                    Handling::Handled(Vec::new())
                } else {
                    Handling::Handled(vec![UiEffect::OsNotification {
                        record,
                        guard,
                        reply,
                    }])
                }
            }
            UiEvent::NotificationFinished(id) => {
                self.delivery_log.clear(&id);
                Handling::Handled(Vec::new())
            }
            UiEvent::ThoughtObserved { runtime, initial } => {
                let generation = self.model.lock().await.conversation_generation();
                Handling::Handled(self.thought.observed(*runtime, initial, generation))
            }
            UiEvent::ThoughtFlushExpired(epoch) => Handling::Handled(self.thought.expired(epoch)),
            UiEvent::ThoughtClear {
                conversation_switch,
            } => {
                self.thought.clear(conversation_switch);
                Handling::Handled(Vec::new())
            }
            UiEvent::ThoughtRequested {
                generation,
                message,
                context,
            } => {
                let config = context.config;
                if context.latest_thought_generation != Some(generation)
                    || context.latest_thought.as_deref() != Some(message.as_str())
                    || !notification::should_show_thought_bubble(
                        config.ui.thought_bubble,
                        context.input_active,
                        main_focused,
                    )
                {
                    self.thought.clear(false);
                    return Handling::Handled(Vec::new());
                }
                let record = BubbleRecord {
                    id: "thought-bubble".into(),
                    created_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    message,
                    message_kind: "thought".into(),
                    notification_priority: "none".into(),
                    caused_by: None,
                    display_name: context.display_name,
                    persona: config.companion.persona.clone(),
                    avatar_color: config.ui.avatar_color.clone(),
                    conversation_generation: generation,
                    persistent: false,
                    interaction: None,
                };
                if register_replacing_for_surface(
                    &self.model,
                    main_focused,
                    record,
                    Duration::from_millis(config.notification.bubble_duration_ms),
                    config.bubble.max_stack,
                    &[],
                )
                .await
                .is_ok()
                {
                    self.refresh().await
                } else {
                    Handling::Handled(Vec::new())
                }
            }
            UiEvent::FactCandidateLoaded(result) => {
                let loaded = match result {
                    Ok(loaded) => loaded,
                    Err(error) => return Handling::Handled(vec![UiEffect::Log(error)]),
                };
                let generation = self.model.lock().await.conversation_generation();
                if generation != loaded.conversation_generation {
                    return Handling::Handled(Vec::new());
                }
                let Some(candidate) = loaded.candidate else {
                    return Handling::Handled(Vec::new());
                };
                let record =
                    crate::presence_presenter::fact_prompt(candidate, &self.config, generation);
                match register_replacing_for_surface(
                    &self.model,
                    main_focused,
                    record,
                    Duration::from_millis(self.config.notification.bubble_duration_ms),
                    self.config.bubble.max_stack,
                    &[],
                )
                .await
                {
                    Ok(_) => self.refresh().await,
                    Err(error) => Handling::Handled(vec![UiEffect::Log(error.to_string())]),
                }
            }
            UiEvent::BubbleRequested {
                record,
                duration_ms,
                replaced_ids,
                reply,
            } => {
                let result = register_replacing_for_surface(
                    &self.model,
                    main_focused,
                    *record,
                    Duration::from_millis(duration_ms),
                    self.config.bubble.max_stack,
                    &replaced_ids,
                )
                .await;
                let _ = reply.send(result);
                Handling::Handled(Vec::new())
            }
            UiEvent::ResetPromptRequested => {
                let config = &self.config;
                let generation = self.model.lock().await.conversation_generation();
                let record = crate::bubble_conversation::reset_prompt_record_for_locale(
                    config,
                    generation,
                    coosenpai_core::locale::Locale::from_config(&config.ui.language),
                );
                match register_replacing_for_surface(
                    &self.model,
                    main_focused,
                    record,
                    Duration::from_millis(config.notification.bubble_duration_ms),
                    config.bubble.max_stack,
                    &[],
                )
                .await
                {
                    Ok(_) => self.refresh().await,
                    Err(error) => Handling::Handled(vec![UiEffect::Fail(error.to_string())]),
                }
            }
            UiEvent::BubbleRendererReady { attempt } => {
                self.observe_conversation_generation().await;
                let (snapshot, display) = self.snapshot().await;
                let setup = snapshot
                    .records
                    .iter()
                    .any(|record| record.message_kind == "setup");
                let log = UiEffect::Log(crate::e2e_logs::renderer_ready(
                    attempt,
                    snapshot.generation,
                    snapshot.records.len(),
                    setup,
                ));
                let Handling::Handled(mut effects) = self.update(snapshot, display) else {
                    unreachable!()
                };
                effects.push(log);
                effects.extend(self.schedule_expiry().await);
                Handling::Handled(effects)
            }
            UiEvent::BubbleRefresh | UiEvent::Mounted(_) => self.refresh().await,
            UiEvent::SnapshotUpdated(snapshot) => {
                self.initialize(&snapshot);
                self.sync_latest_coo_speech(&snapshot).await;
                if self.model.lock().await.sync_feedback_interactions(
                    &snapshot.config,
                    &snapshot.recorded_utterance_feedback_ids,
                ) {
                    self.refresh().await
                } else {
                    self.refresh_appearance().await
                }
            }
            UiEvent::Present(ViewCommand::Hide) => {
                if main_focused {
                    self.model.lock().await.clear_for_main_window();
                }
                let Handling::Handled(mut effects) = self.present(PresentationEvent::Hide) else {
                    unreachable!()
                };
                if main_focused {
                    if let Handling::Handled(render) = self.refresh().await {
                        effects.extend(render);
                    }
                }
                Handling::Handled(effects)
            }
            event => self.handle(event),
        }
    }

    async fn complete_click(&mut self, target: super::BubbleClickTarget) -> Handling {
        let Some(tutorial) = self.tutorial.clone() else {
            return Handling::Handled(vec![UiEffect::Fail(
                "クリックのチュートリアル状態がありません".to_owned(),
            )]);
        };
        let (changed, focus_composer) = {
            let tutorial = tutorial.lock().await;
            let mut model = self.model.lock().await;
            model.expire(Instant::now());
            if !model.accepts_click(&target) {
                return Handling::Handled(Vec::new());
            }
            let focus = tutorial.chat_practice_notice_is_current(&target.record.id)
                && model.accepts_interaction(
                    &target.record.id,
                    crate::tutorial::TUTORIAL_SKIP_ACTION,
                    None,
                );
            (
                model.complete_action_if_not_interactive(&target.record.id),
                focus,
            )
        };
        let mut effects = Vec::new();
        if focus_composer {
            effects.push(UiEffect::Deliver {
                child: PresenterId::Root,
                event: UiEvent::FocusComposer,
            });
        }
        effects.push(UiEffect::Deliver {
            child: PresenterId::Root,
            event: UiEvent::SelectConversation(target.record.id.clone()),
        });
        if changed {
            let Handling::Handled(refresh) = self.refresh().await else {
                unreachable!()
            };
            effects.extend(refresh);
        }
        Handling::Handled(effects)
    }

    async fn refresh(&mut self) -> Handling {
        self.observe_conversation_generation().await;
        let (snapshot, display) = self.snapshot().await;
        let Handling::Handled(mut effects) = self.update(snapshot, display) else {
            unreachable!()
        };
        effects.extend(self.schedule_expiry().await);
        Handling::Handled(effects)
    }

    async fn handle_edge_poll(
        &mut self,
        at_edge: bool,
        config_revision: u64,
        main_focused: bool,
    ) -> Handling {
        if config_revision != self.config.revision {
            self.reset_edge_recall();
            return Handling::Handled(Vec::new());
        }
        self.observe_conversation_generation().await;
        if !at_edge {
            self.reset_edge_recall();
            return Handling::Handled(Vec::new());
        }
        if self.edge_recall_is_suppressed(main_focused).await {
            return Handling::Handled(Vec::new());
        }

        let now = Instant::now();
        let (next, recalled) =
            crate::bubble_edge_recall::observe_edge(self.edge_recall, at_edge, now);
        self.edge_recall = next;
        if !recalled {
            return Handling::Handled(Vec::new());
        }

        let changed = self.model.lock().await.recall_latest(
            now,
            Duration::from_millis(self.config.notification.bubble_duration_ms),
            self.config.bubble.max_stack,
        );
        if changed {
            self.refresh().await
        } else {
            Handling::Handled(Vec::new())
        }
    }

    async fn observe_conversation_generation(&mut self) {
        let generation = self.model.lock().await.conversation_generation();
        if generation != self.conversation_generation {
            self.conversation_generation = generation;
            self.reset_edge_recall();
        }
    }

    async fn onboarding_disables_edge_recall(&mut self) -> bool {
        let current = if let Some(tutorial) = self.tutorial.clone() {
            let tutorial = tutorial.lock().await;
            let state = tutorial.state();
            state.tutorial_active() || state.needs_setup()
        } else {
            false
        };
        if current && !self.onboarding_disables_edge_recall {
            self.reset_edge_recall();
        }
        self.onboarding_disables_edge_recall = current;
        current
    }

    async fn edge_recall_is_suppressed(&mut self, main_focused: bool) -> bool {
        crate::bubble_edge_recall::is_suppressed(
            self.config.bubble.edge_recall,
            main_focused,
            self.onboarding_disables_edge_recall().await,
            self.model.lock().await.can_poll_edge_recall(),
        )
    }

    async fn schedule_expiry(&mut self) -> Vec<UiEffect> {
        let deadline = self.model.lock().await.next_expiry();
        if self.expiry_deadline == deadline {
            return Vec::new();
        }
        self.expiry_deadline = deadline;
        self.expiry_epoch = self.expiry_epoch.saturating_add(1);
        deadline
            .map(|deadline| {
                UiEffect::Spawn(UiTask::Delay {
                    duration: deadline.saturating_duration_since(Instant::now()),
                    event: UiEvent::BubbleExpiry(self.expiry_epoch),
                })
            })
            .into_iter()
            .collect()
    }

    async fn refresh_appearance(&mut self) -> Handling {
        let Some(snapshot) = self.snapshot.clone() else {
            return Handling::Handled(Vec::new());
        };
        let preview = self.model.lock().await.appearance_preview();
        let (snapshot, display) = self.appearance((*snapshot).clone(), preview);
        if self.presentation.state() != crate::presentation::PresentationState::Shown {
            self.snapshot = Some(snapshot);
            self.display = display;
            return Handling::Handled(Vec::new());
        }
        self.render(snapshot, display)
    }

    async fn snapshot(&self) -> (Arc<super::BubbleSnapshot>, String) {
        let mut model = self.model.lock().await;
        model.reconcile_deck(Instant::now());
        self.appearance(model.snapshot(), model.appearance_preview())
    }

    fn appearance(
        &self,
        mut snapshot: super::BubbleSnapshot,
        preview: Option<super::BubbleAppearancePreview>,
    ) -> (Arc<super::BubbleSnapshot>, String) {
        let config = &self.config;
        snapshot.theme = preview
            .as_ref()
            .map_or_else(|| config.ui.theme.clone(), |value| value.theme.clone());
        snapshot.font = preview
            .as_ref()
            .map_or_else(|| config.ui.font.clone(), |value| value.font.clone());
        snapshot.avatar_color = preview
            .as_ref()
            .map(|value| value.avatar_color.clone())
            .or_else(|| config.ui.avatar_color.clone());
        snapshot.position = preview.as_ref().map_or_else(
            || config.bubble.position.clone(),
            |value| value.position.clone(),
        );
        snapshot.language = config.ui.language.clone();
        snapshot.avatar_image_png = self.avatar_image_png.clone();
        let display = preview.map_or_else(|| config.bubble.display.clone(), |value| value.display);
        (Arc::new(snapshot), display)
    }
}

pub(crate) async fn register_replacing_for_surface(
    bubbles: &Mutex<BubbleState>,
    main_window_focused: bool,
    record: BubbleRecord,
    duration: Duration,
    max_stack: usize,
    replaced_ids: &[String],
) -> Result<BubblePresentation> {
    let id = record.id.clone();
    let mut bubbles = bubbles.lock().await;
    if record.conversation_generation != bubbles.conversation_generation() {
        anyhow::bail!("会話世代が切り替わったため吹き出しを破棄しました");
    }
    if main_window_focused && record.interaction.is_none() {
        return Ok(acknowledged_in_main());
    }
    let shown = if replaced_ids.is_empty() {
        bubbles.show(record, Instant::now(), duration, max_stack)
    } else {
        bubbles.show_replacing(record, Instant::now(), duration, max_stack, replaced_ids)
    };
    if !shown {
        anyhow::bail!("会話リセット前の吹き出しです");
    }
    let dismissed = bubbles
        .presentation_cancellation(&id)
        .context("登録した吹き出しの表示状態がありません")?;
    let explicitly_dismissed = bubbles
        .entries
        .iter()
        .find(|entry| entry.record.id == id)
        .context("登録した吹き出しの却下状態がありません")?
        .explicitly_dismissed
        .clone();
    Ok(BubblePresentation {
        explicitly_dismissed,
        generation: bubbles.generation,
        acknowledgements: bubbles.subscribe_acknowledgements(),
        dismissed,
        registered_on_bubble_surface: true,
    })
}

fn acknowledged_in_main() -> BubblePresentation {
    let (_, acknowledgements) = watch::channel(0);
    BubblePresentation {
        explicitly_dismissed: CancellationToken::new(),
        generation: 0,
        acknowledgements,
        dismissed: CancellationToken::new(),
        registered_on_bubble_surface: false,
    }
}

fn latest_coo_speech(snapshot: &crate::snapshot::AppSnapshot) -> Option<BubbleRecord> {
    let tutorial_input_ids = snapshot
        .conversation
        .iter()
        .filter(|entry| {
            entry.role == coosenpai_core::state::ConversationRole::User
                && entry.tutorial_response_key.is_some()
        })
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let normal_user_input_ids = snapshot
        .conversation
        .iter()
        .filter(|entry| entry.is_normal_user_input())
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    snapshot.conversation.iter().rev().find_map(|entry| {
        if entry
            .caused_by_ids
            .iter()
            .any(|id| tutorial_input_ids.contains(id))
        {
            return None;
        }
        let (message_kind, interaction) = if entry.is_normal_speech() {
            let message_kind = entry.message_kind?.as_wire().to_owned();
            let interaction = crate::utterance_feedback::interaction_for_speech(
                &snapshot.config,
                &message_kind,
                snapshot.recorded_utterance_feedback_ids.contains(&entry.id),
            );
            (message_kind, interaction)
        } else if entry.role == coosenpai_core::state::ConversationRole::Companion
            && entry.message_kind.is_none()
            && entry.tutorial_response_key.is_none()
            && !entry.message.trim().is_empty()
            && entry
                .caused_by_ids
                .iter()
                .any(|id| normal_user_input_ids.contains(id))
        {
            // 旧形式は表示・再表示だけを維持し、評価対象にはしない。
            ("chat".to_owned(), None)
        } else {
            return None;
        };
        Some(BubbleRecord {
            id: entry.id.clone(),
            created_at: entry.created_at.clone(),
            message: entry.message.clone(),
            message_kind,
            notification_priority: entry.notification_priority.clone(),
            caused_by: entry.caused_by_ids.last().cloned(),
            display_name: snapshot.companion_display_name.clone(),
            persona: snapshot.config.companion.persona.clone(),
            avatar_color: snapshot.config.ui.avatar_color.clone(),
            conversation_generation: snapshot.selected_conversation_generation,
            persistent: false,
            interaction,
        })
    })
}

