use crate::capture::CaptureKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PresenterId {
    Root,
    Tutorial,
    Tray,
    Chat,
    Capture,
    Bubble,
    Details,
    Settings,
    ModelPicker,
    Avatar,
}

impl PresenterId {
    pub(crate) fn attached_windows(self) -> &'static [Self] {
        match self {
            Self::Chat => &[Self::Settings, Self::ModelPicker],
            _ => &[],
        }
    }

    pub(crate) fn parent(self) -> Option<Self> {
        match self {
            Self::Root => None,
            Self::Tutorial => Some(Self::Chat),
            Self::Tray
            | Self::Chat
            | Self::Capture
            | Self::Bubble
            | Self::Details
            | Self::Settings
            | Self::ModelPicker
            | Self::Avatar => Some(Self::Root),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UiView {
    Application,
    GlobalShortcut,
    Chat,
    CapturePopup,
    SpeechPopup,
    Bubble,
    Details,
    Settings,
    ModelPicker,
    Avatar,
}

impl UiView {
    pub(crate) fn presenter(self) -> PresenterId {
        match self {
            Self::Application | Self::GlobalShortcut => PresenterId::Root,
            Self::Chat => PresenterId::Chat,
            Self::CapturePopup | Self::SpeechPopup => PresenterId::Capture,
            Self::Bubble => PresenterId::Bubble,
            Self::Details => PresenterId::Details,
            Self::Settings => PresenterId::Settings,
            Self::ModelPicker => PresenterId::ModelPicker,
            Self::Avatar => PresenterId::Avatar,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewCommand {
    Show,
    Hide,
    Front,
    FocusInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VoiceAction {
    Start(crate::speech::SpeechSource),
    Finish,
    Cancel,
}

#[derive(Debug)]
pub(crate) enum ChatInput {
    Capture {
        content: std::sync::Arc<crate::capture::ReadyCapture>,
        message: String,
    },
    Voice {
        generation: u64,
        text: String,
    },
}

#[derive(Debug)]
pub(crate) enum ChatProjection {
    Update(crate::app_update::UpdateSnapshot),
    VoiceOutput(crate::voice_output::VoiceOutputSnapshot),
}

#[derive(Debug)]
pub(crate) enum UiEvent {
    SettingsPreview {
        preview: Option<crate::bubbles::BubbleAppearancePreview>,
        reply: tokio::sync::oneshot::Sender<crate::commands::IpcResult<()>>,
    },
    SettingsPreviewCompleted(crate::commands::IpcResult<()>),
    Panel {
        owner: PresenterId,
        request: crate::panels::PanelRequest,
        reply: tokio::sync::oneshot::Sender<Result<crate::panels::PanelOutput, String>>,
    },
    Tutorial(Box<crate::tutorial_events::TutorialEvent>),
    PresenceTickCompleted {
        date: chrono::NaiveDate,
        result: Result<bool, String>,
    },
    FactCandidateLoaded(Result<crate::presence_presenter::FactCandidateLoaded, String>),
    BubbleExpiry(u64),
    ThoughtObserved {
        runtime: Box<coosenpai_core::runtime::RuntimeSnapshot>,
        initial: bool,
    },
    ThoughtFlushExpired(u64),
    ThoughtClear {
        conversation_switch: bool,
    },
    TrayMounted,
    TrayReady(std::sync::Arc<crate::snapshot::AppSnapshot>),
    WindowHideFailed {
        view: PresenterId,
        error: String,
    },
    SnapshotCompleted(Box<crate::snapshot_presenter::SnapshotEvent>),
    SpeechFailureExpired {
        generation: u64,
        failure_id: u64,
    },
    SnapshotResult(Box<crate::snapshot_presenter::SnapshotInput>),
    Placement(crate::placement_presenter::PlacementEvent),
    UserCommand(crate::ui_commands::UserCommand),
    CommandFinished {
        owner: PresenterId,
        completion: crate::ui_commands::CommandCompletion,
    },
    ModelSaved {
        generation: u64,
        result: Box<crate::commands::IpcResult<coosenpai_core::config::Config>>,
        reply: crate::ui_commands::Reply<coosenpai_core::config::Config>,
    },
    ChatInputActive(bool),
    UnreadRead,
    BubbleAck {
        generation: u64,
        reply: crate::ui_commands::Reply<()>,
    },
    BubbleSnapshot(crate::ui_commands::Reply<crate::bubbles::BubbleSnapshot>),

    ThoughtRequested {
        generation: u64,
        message: String,
        context: Box<crate::bubbles::presenter::NotificationContext>,
    },
    NotificationRequested {
        record: coosenpai_core::notification::NotificationRecord,
        target: crate::state::NotificationTarget,
        context: Box<crate::bubbles::presenter::NotificationContext>,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    BubbleRequested {
        record: Box<crate::bubbles::BubbleRecord>,
        duration_ms: u64,
        replaced_ids: Vec<String>,
        reply: tokio::sync::oneshot::Sender<anyhow::Result<crate::bubbles::BubblePresentation>>,
    },
    OsNotificationPrepared {
        record: coosenpai_core::notification::NotificationRecord,
        guard: Option<tokio::sync::OwnedMutexGuard<()>>,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    NotificationFinished(String),
    ResetPromptRequested,
    BubbleMutation {
        mutation: crate::bubbles::BubbleMutation,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    BubbleRefresh,
    BubbleRendererReady {
        attempt: u32,
    },
    BubbleWindow(crate::bubbles::presenter::BubbleWindowEvent),
    AvatarWindow(crate::presentation::PresentationEvent<(), ()>),
    AvatarRead {
        model_changed: bool,
        reply: tokio::sync::oneshot::Sender<crate::avatar_presenter::AvatarState>,
    },
    AvatarRenderFailed {
        revision: u64,
        error: String,
    },
    AvatarApplied {
        visible: bool,
        generation: u64,
        result: Result<(), String>,
    },
    ChatProjection(ChatProjection),
    GlobalShortcut {
        shortcut: String,
        pressed: bool,
    },
    ShortcutResolved {
        action: Option<crate::capture::ShortcutAction>,
        pressed: bool,
        voice_mode: String,
        popup_generation: Option<u64>,
    },
    Shortcut(CaptureKind),
    Activation(crate::activation_policy::ActivationInput),
    ApplicationDeactivated,
    OtherApplicationActivated,
    Voice(VoiceAction),
    PopupProtection {
        generation: u64,
        active: bool,
    },
    OperationStarted {
        kind: CaptureKind,
        generation: u64,
    },
    OperationEnded {
        kind: CaptureKind,
        generation: u64,
    },
    BubbleHideExpired(u64),
    BubbleTypingTick(u64),
    BubbleResize(u32),
    BubbleClick {
        id: String,
        body: bool,
    },
    BubbleClickPrepared {
        generation: u64,
        target: crate::bubbles::BubbleClickTarget,
        result: Result<Option<crate::ui_load::WindowContent>, String>,
    },
    BubbleClickCompleted(crate::bubbles::BubbleClickTarget),
    FocusComposer,
    SelectConversation(String),
    BubbleFocused(bool),
    RequestFocus,
    PointerPassthrough,
    RestorePointer,
    AvatarVisibility(Option<bool>),
    EmptyClipboard,
    OpenMain,
    ToggleMain,
    OpenSettings,
    OpenSettingsAt(&'static str),
    OpenDetails,
    OpenModelPicker,
    Window {
        view: PresenterId,
        event: crate::ui_load::WindowEvent,
    },
    Refreshed {
        view: PresenterId,
        result: Result<crate::ui_load::WindowContent, String>,
    },
    EffectCompleted(Result<EffectResult, String>),
    CaptureCompleted(Box<crate::capture::CaptureEvent>),
    PopupWindow {
        view: UiView,
        event: crate::presentation::PresentationEvent<(), ()>,
    },
    Close,
    Present(ViewCommand),
    Mounted(UiView),
    SpeechTranscript {
        generation: u64,
        text: String,
    },
    SubmitChat(String),
    SubmitInput(ChatInput),
    MainVisibility(bool),
    MainFocused(bool),
    CaptureCancel {
        generation: u64,
        source: crate::capture::CancelSource,
    },
    SnapshotUpdated(std::sync::Arc<crate::snapshot::AppSnapshot>),
    InterruptCapture(bool),
    NativeShutdown(crate::shutdown::ExitKind),
    Shutdown,
    #[cfg(test)]
    Unhandled,
    ModelPicker(crate::model_picker_presenter::ModelPickerEvent),
    BubbleView(crate::bubble_controls_presenter::BubbleViewEvent),
    Composer(crate::composer_presenter::ComposerEvent),
    App(crate::app_presenter::AppEvent),
    AvatarScene(crate::avatar_scene_presenter::AvatarSceneInput),
    AvatarMotionsChanged,
    AvatarPreview(crate::motion_settings_presenter::PreviewRequest),
    MotionSettings(crate::motion_settings_presenter::MotionEvent),
    StatusDeadline(crate::status_presenter::StatusDeadline),
    WorkApproval(crate::work_approval_presenter::WorkApprovalEvent),
    Conversation(crate::conversation_presenter::ConversationEvent),
    ChatLoaded(std::sync::Arc<crate::snapshot::AppSnapshot>),
}

#[derive(Debug)]
pub(crate) enum UiEffect {
    PanelOutput {
        reply: tokio::sync::oneshot::Sender<Result<crate::panels::PanelOutput, String>>,
        output: Result<crate::panels::PanelOutput, String>,
    },
    TrayRender(Box<crate::tray_presenter::TrayView>),
    ChatInputActive(bool),
    OsNotification {
        record: coosenpai_core::notification::NotificationRecord,
        guard: tokio::sync::OwnedMutexGuard<()>,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    AvatarRender(crate::avatar_presenter::AvatarState),
    AvatarVisibility {
        visible: bool,
        generation: u64,
        position_initial: bool,
        language: String,
    },
    ChatProjection(ChatProjection),
    Activation(crate::activation_policy::ActivationInput),
    ActivationView(crate::activation_policy::ActivationView),
    CaptureView {
        effects: Vec<crate::capture::effects::CaptureEffect>,
        completion: crate::capture::effects::CaptureOutput,
    },
    Spawn(UiTask),
    Run(UiTask),
    Complete(Result<EffectResult, String>),
    Fail(String),
    RenderWindow(crate::ui_load::WindowContent),
    BubbleTyping {
        id: String,
        revealed: Option<usize>,
    },
    BubbleRender {
        snapshot: std::sync::Arc<crate::bubbles::BubbleSnapshot>,
        display: String,
        layout: bool,
    },
    BubbleResize {
        height: u32,
        position: String,
        display: String,
    },
    BubbleFocused(bool),
    Pointer(bool),
    SelectConversation(String),
    Deliver {
        child: PresenterId,
        event: UiEvent,
    },
    View {
        view: PresenterId,
        command: ViewCommand,
    },
    SettingsFocus(&'static str),
    MainFocus(bool),
    RenderSnapshot {
        view: PresenterId,
        snapshot: std::sync::Arc<crate::snapshot::AppSnapshot>,
    },
    ForceShutdown,
    RejectInput,
    Log(String),
    WorkApprovalRender(Box<crate::work_approval_presenter::WorkApprovalView>),
    AppRender(Box<crate::app_presenter::AppView>),
    StatusRender(Box<crate::status_presenter::StatusView>),
    PersonasRender(Vec<crate::factory::PersonaOption>),
    AvatarSceneRender(Box<crate::avatar_scene_presenter::AvatarSceneView>),
    MotionSettingsRender(Box<crate::motion_settings_presenter::MotionView>),
    MotionStorage(crate::motion_settings_presenter::MotionStorage),
    ModelPickerRender(Box<crate::model_picker_presenter::ModelPickerView>),
    BubbleControls(Box<crate::bubble_controls_presenter::BubbleControlsView>),
    ComposerRender(Box<crate::composer_presenter::ComposerView>),
    ConversationRender(Box<crate::conversation_presenter::ConversationView>),
}

#[derive(Debug)]
pub(crate) enum UiTask {
    SettingsPreview(Option<crate::bubbles::BubbleAppearancePreview>),
    Tutorial(crate::tutorial_events::TutorialTask),
    RefreshConversation,
    ReadFactCandidate(chrono::NaiveDate),
    RuntimeFollowup,
    ReadThoughtPresentation {
        generation: u64,
        message: String,
    },
    ClearTemporary {
        expires_at: chrono::DateTime<chrono::Utc>,
    },
    LoadAvatar {
        generation: u64,
        path: Option<String>,
    },
    SavePlacement {
        path: std::path::PathBuf,
        placement: crate::placement_presenter::MainWindowPlacement,
    },
    SaveModel {
        patch: serde_json::Value,
        generation: u64,
        reply: crate::ui_commands::Reply<coosenpai_core::config::Config>,
    },
    DismissBubble {
        id: String,
        restart_setup: bool,
        reply: crate::ui_commands::Reply<()>,
    },
    UserCommand(crate::ui_commands::UserCommand),
    Capture {
        effect: crate::capture::effects::CaptureEffect,
        completion: crate::capture::effects::CaptureTask,
    },
    CompleteNotification {
        presentation: crate::bubbles::BubblePresentation,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    PrepareOsNotification {
        record: coosenpai_core::notification::NotificationRecord,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Load {
        view: PresenterId,
        generation: u64,
        request: crate::ui_load::WindowRequest,
    },
    Refresh(PresenterId),
    Delay {
        duration: std::time::Duration,
        event: UiEvent,
    },
    EmptyClipboardNotice(crate::capture_notice::NoticeTarget),
    BubbleClick {
        generation: u64,
        target: crate::bubbles::BubbleClickTarget,
    },
    BubbleFastForward(String),
    ResolveShortcut {
        shortcut: String,
        pressed: bool,
    },
    ToggleWatch,
    CopyLastReply,
    StartOperation {
        kind: CaptureKind,
        source: crate::command_guard::CommandSource,
    },
    Activation(crate::activation_policy::ActivationTask),
    StopOperation {
        kind: CaptureKind,
        generation: u64,
    },
    Voice(VoiceAction),
    SubmitChat(String),
    SubmitInput(ChatInput),
    AnnounceSetup,
    MainOpened,
    InterruptCapture(bool),
    NativeShutdown(crate::shutdown::ExitKind),
    Shutdown,
    ModelPicker(crate::model_picker_presenter::ModelPickerTask),
    Composer(crate::composer_presenter::ComposerTask),
    App(crate::app_presenter::AppTask),
    MotionSettings(crate::motion_settings_presenter::MotionTask),
    StatusDelay(crate::status_presenter::StatusDeadline),
    WorkApproval(crate::work_approval_presenter::WorkApprovalTask),
    BubbleView {
        id: String,
        token: u64,
        action: Option<String>,
        value: Option<String>,
        restart_setup: bool,
    },
    Conversation(crate::conversation_presenter::ConversationTask),
}

impl UiEvent {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::SettingsPreview { .. } => "SettingsPreview".into(),
            Self::SettingsPreviewCompleted(_) => "SettingsPreviewCompleted".into(),
            Self::Panel { owner, .. } => format!("Panel({owner:?})"),
            Self::ModelPicker(_) => "ModelPicker".into(),
            Self::BubbleView(crate::bubble_controls_presenter::BubbleViewEvent::Input(input)) => {
                format!("BubbleView({})", input.label())
            }
            Self::BubbleView(crate::bubble_controls_presenter::BubbleViewEvent::Completed {
                ..
            }) => "BubbleView(Completed)".into(),
            Self::AvatarScene(_) => "AvatarScene".into(),
            Self::AvatarMotionsChanged => "AvatarMotionsChanged".into(),
            Self::AvatarPreview(_) => "AvatarPreview".into(),
            Self::MotionSettings(_) => "MotionSettings".into(),
            Self::App(_) => "App".into(),
            Self::StatusDeadline(_) => "StatusDeadline".into(),
            Self::WorkApproval(_) => "WorkApproval".into(),
            Self::Composer(_) => "Composer".into(),
            Self::Conversation(_) => "Conversation".into(),
            Self::ChatLoaded(_) => "ChatLoaded".into(),
            Self::UserCommand(command) => format!("UserCommand({:?})", command.owner()),
            Self::CommandFinished { owner, .. } => format!("CommandFinished({owner:?})"),
            Self::ModelSaved { .. } => "ModelSaved".into(),
            Self::ChatInputActive(active) => format!("ChatInputActive({active})"),
            Self::UnreadRead => "UnreadRead".into(),
            Self::BubbleAck { generation, .. } => format!("BubbleAck({generation})"),
            Self::BubbleSnapshot(_) => "BubbleSnapshot".into(),
            Self::OsNotificationPrepared { .. } => "OsNotificationPrepared".into(),
            Self::NotificationFinished(_) => "NotificationFinished".into(),
            Self::NotificationRequested { .. } => "NotificationRequested".into(),
            Self::ThoughtRequested { .. } => "ThoughtRequested".into(),
            Self::BubbleRequested { .. } => "BubbleRequested".into(),
            Self::ResetPromptRequested => "ResetPromptRequested".into(),
            Self::BubbleMutation { .. } => "BubbleMutation".into(),
            Self::BubbleWindow(_) => "BubbleWindow".into(),
            Self::BubbleRefresh => "BubbleRefresh".into(),
            Self::BubbleRendererReady { attempt } => format!("BubbleRendererReady({attempt})"),
            Self::AvatarWindow(_) => "AvatarWindow".into(),
            Self::AvatarRead { .. } => "AvatarRead".into(),
            Self::AvatarRenderFailed { revision, .. } => format!("AvatarRenderFailed({revision})"),
            Self::AvatarApplied { .. } => "AvatarApplied".into(),
            Self::ChatProjection(_) => "ChatProjection".into(),
            Self::GlobalShortcut { pressed, .. } => format!("GlobalShortcut(pressed={pressed})"),
            Self::ShortcutResolved {
                action, pressed, ..
            } => format!("ShortcutResolved({action:?},pressed={pressed})"),
            Self::Shortcut(kind) => format!("Shortcut({kind:?})"),
            Self::Activation(_) => "Activation".into(),
            Self::ApplicationDeactivated => "ApplicationDeactivated".into(),
            Self::OtherApplicationActivated => "OtherApplicationActivated".into(),
            Self::Voice(action) => format!("Voice({action:?})"),
            Self::PopupProtection { generation, active } => {
                format!("PopupProtection({generation},{active})")
            }
            Self::OperationStarted { kind, generation } => {
                format!("OperationStarted({kind:?},generation={generation})")
            }
            Self::OperationEnded { kind, generation } => {
                format!("OperationEnded({kind:?},generation={generation})")
            }
            Self::BubbleClick { .. } => "BubbleClick".into(),
            Self::BubbleClickPrepared { .. } => "BubbleClickPrepared".into(),
            Self::BubbleClickCompleted(_) => "BubbleClickCompleted".into(),
            Self::FocusComposer => "FocusComposer".into(),
            Self::SelectConversation(_) => "SelectConversation".into(),
            Self::BubbleTypingTick(epoch) => format!("BubbleTypingTick({epoch})"),
            Self::BubbleHideExpired(generation) => format!("BubbleHideExpired({generation})"),
            Self::BubbleResize(_) => "BubbleResize".into(),
            Self::RequestFocus => "RequestFocus".into(),
            Self::BubbleFocused(focused) => format!("BubbleFocused({focused})"),
            Self::PointerPassthrough => "PointerPassthrough".into(),
            Self::RestorePointer => "RestorePointer".into(),
            Self::AvatarVisibility(_) => "AvatarVisibility".into(),
            Self::EmptyClipboard => "EmptyClipboard".into(),
            Self::OpenMain => "OpenMain".into(),
            Self::ToggleMain => "ToggleMain".into(),
            Self::OpenSettingsAt(_) => "OpenSettingsAt".into(),
            Self::OpenSettings => "OpenSettings".into(),
            Self::OpenDetails => "OpenDetails".into(),
            Self::OpenModelPicker => "OpenModelPicker".into(),
            Self::Window { view, event } => format!(
                "Window({view:?},{})",
                match event {
                    crate::presentation::PresentationEvent::Open(_) => "Open".to_owned(),
                    crate::presentation::PresentationEvent::Loaded { generation, .. } =>
                        format!("Loaded({generation})"),
                    crate::presentation::PresentationEvent::Hide => "Hide".to_owned(),
                    crate::presentation::PresentationEvent::CancelLoad { generation } =>
                        format!("CancelLoad({generation})"),
                }
            ),
            Self::Refreshed { view, .. } => format!("Refreshed({view:?})"),
            Self::EffectCompleted(_) => "EffectCompleted".into(),
            Self::CaptureCompleted(event) => format!("CaptureCompleted({event:?})"),
            Self::PopupWindow { view, event } => format!(
                "PopupWindow({view:?},{})",
                match event {
                    crate::presentation::PresentationEvent::Open(()) => "Open".to_owned(),
                    crate::presentation::PresentationEvent::Loaded { generation, .. } =>
                        format!("Loaded({generation})"),
                    crate::presentation::PresentationEvent::Hide => "Hide".to_owned(),
                    crate::presentation::PresentationEvent::CancelLoad { generation } =>
                        format!("CancelLoad({generation})"),
                }
            ),
            Self::Close => "Close".into(),
            Self::Present(command) => format!("Present({command:?})"),
            Self::Mounted(view) => format!("Mounted({view:?})"),
            Self::SpeechTranscript { generation, .. } => format!("SpeechTranscript({generation})"),
            Self::SubmitInput(_) => "SubmitInput".into(),
            Self::SubmitChat(_) => "SubmitChat".into(),
            Self::MainVisibility(visible) => format!("MainVisibility({visible})"),
            Self::MainFocused(focused) => format!("MainFocused({focused})"),
            Self::CaptureCancel { generation, .. } => {
                format!("CaptureCancel(generation={generation})")
            }
            Self::SpeechFailureExpired {
                generation,
                failure_id,
            } => format!("SpeechFailureExpired({generation}, {failure_id})"),
            Self::Tutorial(event) => event.label(),
            Self::PresenceTickCompleted { .. } => "PresenceTickCompleted".to_owned(),
            Self::FactCandidateLoaded(_) => "FactCandidateLoaded".to_owned(),
            Self::BubbleExpiry(epoch) => format!("BubbleExpiry({epoch})"),
            Self::ThoughtObserved { .. } => "ThoughtObserved".to_owned(),
            Self::ThoughtFlushExpired(epoch) => format!("ThoughtFlushExpired({epoch})"),
            Self::ThoughtClear { .. } => "ThoughtClear".to_owned(),
            Self::TrayReady(_) => "TrayReady".into(),
            Self::WindowHideFailed { view, .. } => format!("WindowHideFailed({view:?})"),
            Self::TrayMounted => "TrayMounted".to_owned(),
            Self::SnapshotCompleted(_) => "SnapshotCompleted".to_owned(),
            Self::SnapshotResult(_) => "SnapshotResult".to_owned(),
            Self::Placement(event) => format!("Placement({event:?})"),
            Self::SnapshotUpdated(snapshot) => {
                format!("SnapshotUpdated(revision={})", snapshot.revision)
            }
            Self::InterruptCapture(selection_only) => {
                format!("InterruptCapture(selection_only={selection_only})")
            }
            Self::NativeShutdown(kind) => format!("NativeShutdown({kind:?})"),
            Self::Shutdown => "Shutdown".into(),
            #[cfg(test)]
            Self::Unhandled => "Unhandled".into(),
        }
    }
}

pub(crate) enum Handling {
    Handled(Vec<UiEffect>),
    Bubble(UiEvent),
}

#[derive(Debug)]
pub(crate) struct EffectResult {
    pub(crate) events: Vec<UiEvent>,
    pub(crate) value: Option<String>,
}

impl EffectResult {
    pub(crate) fn done() -> Self {
        Self {
            events: Vec::new(),
            value: None,
        }
    }
}
