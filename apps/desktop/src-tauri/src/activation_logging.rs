use super::runtime::{ActivationEffect, HandoffEffect};
use super::ActivationInput;
use crate::ui_events::{UiEffect, UiTask};

pub(super) fn input_label(input: &ActivationInput) -> String {
    use ActivationInput as I;
    match input {
        I::MainWindow(command) => format!("MainWindow({command:?})"),
        I::MainShown(origin) => format!("MainShown(origin={})", origin.is_some()),
        I::Open(open) => format!("Open(generation={},kind={:?})", open.generation, open.kind),
        I::Supersede(generation) => format!("Supersede({generation})"),
        I::Prepared { generation, result } => {
            format!("Prepared({generation},ok={})", result.is_ok())
        }
        I::Returned { generation, result } => {
            format!("Returned({generation},ok={})", result.is_ok())
        }
        I::Popup { generation, .. } => format!("Popup({generation})"),
        I::PopupFramePrepared {
            generation, result, ..
        } => format!("PopupFramePrepared({generation},ok={})", result.is_ok()),
        I::PopupShown { generation, result } => {
            format!("PopupShown({generation},ok={})", result.is_ok())
        }
        I::HidePopup => "HidePopup".into(),
        I::PopupClosed {
            generation,
            sent,
            restarting,
            ..
        } => format!("PopupClosed({generation},sent={sent},restarting={restarting})"),
        I::PopupHideFailed(generation) => format!("PopupHideFailed({generation})"),
        I::Deactivated => "Deactivated".into(),
        I::OtherApplicationActivated => "OtherApplicationActivated".into(),
        I::PopupKeyLost(generation) => format!("PopupKeyLost({generation})"),
        I::PopupKeyReacquired {
            generation,
            attempt,
            result,
        } => format!(
            "PopupKeyReacquired({generation},attempt={attempt},ok={})",
            matches!(result, Ok(true))
        ),
        I::PopupFailed(generation) => format!("PopupFailed({generation})"),
        I::NavigationFailed(_) => "NavigationFailed".into(),
    }
}

pub(super) fn view_label(effect: &ActivationEffect) -> &'static str {
    match effect {
        ActivationEffect::MainWindow { .. } => "MainWindow",
        ActivationEffect::ShowPopup { .. } => "ShowPopup",
        ActivationEffect::ReacquirePopupKey { .. } => "ReacquirePopupKey",
        ActivationEffect::RestoreOrigin(_) => "RestoreOrigin",
    }
}

pub(super) fn handoff_label(effect: &HandoffEffect) -> &'static str {
    match effect {
        HandoffEffect::PrepareOrigin(_) => "PrepareOrigin",
        HandoffEffect::ReturnToOrigin(_) => "ReturnToOrigin",
    }
}

pub(super) fn effect_label(effect: &UiEffect) -> String {
    match effect {
        UiEffect::ActivationView(view) => view_label(&view.effect).into(),
        UiEffect::Spawn(UiTask::Activation(task)) => handoff_label(&task.effect).into(),
        UiEffect::Spawn(UiTask::Capture { .. }) => "OpenSelection".into(),
        UiEffect::CaptureView { .. } => "PreparePopupFrame".into(),
        UiEffect::Deliver { child, event } => format!("Deliver({child:?},{})", event.label()),
        UiEffect::View { view, command } => format!("View({view:?},{command:?})"),
        UiEffect::Activation(input) => input_label(input),
        UiEffect::Log(message) => message.clone(),
        _ => unreachable!("ActivationPolicy returned an unlabelled effect"),
    }
}

pub(super) fn result_label(
    result: &Result<crate::ui_events::EffectResult, String>,
) -> &'static str {
    let Ok(result) = result else {
        return "error";
    };
    for event in &result.events {
        if matches!(
            event,
            crate::ui_events::UiEvent::Activation(
                ActivationInput::PopupKeyReacquired {
                    result: Ok(false) | Err(_),
                    ..
                } | ActivationInput::PopupShown { result: Err(_), .. }
                    | ActivationInput::Prepared { result: Err(_), .. }
                    | ActivationInput::Returned { result: Err(_), .. }
            )
        ) {
            return "error";
        }
    }
    "ok"
}

