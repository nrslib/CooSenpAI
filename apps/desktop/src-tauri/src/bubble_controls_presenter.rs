use crate::bubbles::{BubbleDeckDirection, BubbleRecord, BubbleSnapshot};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum BubbleViewInput {
    Pointer {
        id: String,
        control: bool,
    },
    Body {
        id: String,
        control: bool,
    },
    Key {
        id: String,
        key: String,
        composing: bool,
        key_code: u32,
        control: bool,
    },
    Dismiss {
        id: String,
    },
    Select {
        id: String,
        value: String,
    },
    Secret {
        id: String,
        value: String,
    },
    Action {
        id: String,
        action: String,
    },
    Navigate {
        direction: BubbleDeckDirection,
    },
    Wheel {
        delta: f64,
        control: bool,
        scroll_top: f64,
        client_height: f64,
        scroll_height: f64,
    },
}

impl BubbleViewInput {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Pointer { .. } => "Pointer",
            Self::Body { .. } => "Body",
            Self::Key { .. } => "Key",
            Self::Dismiss { .. } => "Dismiss",
            Self::Select { .. } => "Select",
            Self::Secret { .. } => "Secret",
            Self::Action { .. } => "Action",
            Self::Navigate { .. } => "Navigate",
            Self::Wheel { .. } => "Wheel",
        }
    }
}

#[derive(Debug)]
pub(crate) enum BubbleViewEvent {
    Input(BubbleViewInput),
    Completed {
        token: u64,
        id: String,
        result: crate::commands::IpcResult<()>,
    },
}

#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BubbleControlsView {
    pub record: Option<BubbleRecord>,
    pub exiting: bool,
    pub reading: bool,
    pub busy: bool,
    pub dismissible: bool,
    pub body_button: bool,
    pub older_edge: bool,
    pub second_edge: bool,
    pub show_latest: bool,
    pub navigation_disabled: bool,
    pub required_input: bool,
    pub error: Option<String>,
    pub clear_secret: u64,
    pub focus_request: u64,
}

pub(crate) enum BubbleControlAction {
    Focus,
    Body {
        id: String,
        open_main: bool,
    },
    Navigate(BubbleDeckDirection),
    Dismiss {
        id: String,
        token: u64,
    },
    Interact {
        id: String,
        token: u64,
        action: String,
        value: Option<String>,
    },
}

#[derive(Default)]
pub(crate) struct BubbleControlsPresenter {
    pub view: BubbleControlsView,
    pub ignored_reason: &'static str,
    last_wheel: Option<Instant>,
    operation: u64,
    secret_operation: bool,
}

impl BubbleControlsPresenter {
    pub(crate) fn observe(&mut self, snapshot: &BubbleSnapshot) {
        let front = snapshot
            .records
            .iter()
            .find(|r| Some(&r.id) == snapshot.front_id.as_ref());
        if front.map(|r| (&r.id, &r.interaction))
            != self.view.record.as_ref().map(|r| (&r.id, &r.interaction))
        {
            self.operation += 1;
            self.view.busy = false;
            self.view.error = None;
            self.view.required_input = false;
        }
        self.view.exiting = front.is_none() && self.view.record.is_some();
        if let Some(front) = front {
            self.view.record = Some(front.clone());
        }
        self.view.reading = snapshot.reading && front.is_some();
        let fixed = front.is_some_and(|r| r.interaction.is_some());
        let index = snapshot
            .history_ids
            .iter()
            .position(|id| Some(id) == snapshot.front_id.as_ref());
        self.view.dismissible = front.is_some_and(|r| r.message_kind != "tutorial");
        self.view.body_button =
            front.is_some_and(|r| r.interaction.is_none() || r.message_kind == "tutorial");
        self.view.older_edge = index.is_some_and(|i| i > 0);
        self.view.second_edge = index.is_some_and(|i| i > 1);
        self.view.show_latest = !fixed && index.is_some_and(|i| i + 1 < snapshot.history_ids.len());
        self.view.navigation_disabled = fixed || front.is_none();
    }

    pub(crate) fn hide(&mut self) {
        self.operation += 1;
        self.view.record = None;
        self.view.busy = false;
        self.view.exiting = false;
    }

    pub(crate) fn complete(
        &mut self,
        token: u64,
        id: &str,
        result: crate::commands::IpcResult<()>,
    ) {
        if token != self.operation || self.view.record.as_ref().is_none_or(|r| r.id != id) {
            return;
        }
        self.view.busy = false;
        match result {
            crate::commands::IpcResult::Success { .. } => {
                self.view.error = None;
                if self.secret_operation {
                    self.view.clear_secret += 1;
                }
            }
            crate::commands::IpcResult::Failure { error, .. } => {
                self.view.error = Some(error.message)
            }
        }
    }

    pub(crate) fn input(
        &mut self,
        input: BubbleViewInput,
        now: Instant,
    ) -> Vec<BubbleControlAction> {
        use BubbleControlAction as Action;
        use BubbleViewInput as Input;
        self.ignored_reason = "unavailable-control";
        let Some(record) = self.view.record.clone().filter(|_| !self.view.exiting) else {
            self.ignored_reason = "no-active-card";
            return vec![];
        };
        let id = match &input {
            Input::Pointer { id, .. }
            | Input::Body { id, .. }
            | Input::Key { id, .. }
            | Input::Dismiss { id }
            | Input::Select { id, .. }
            | Input::Secret { id, .. }
            | Input::Action { id, .. } => Some(id),
            _ => None,
        };
        if id.is_some_and(|id| *id != record.id) {
            self.ignored_reason = "stale-card";
            return vec![];
        }
        match input {
            Input::Pointer { control, .. } => {
                if !control && record.interaction.is_some() && record.message_kind == "tutorial" {
                    self.ignored_reason = "tutorial-body-keeps-focus";
                    return vec![];
                }
                if !control {
                    self.view.focus_request += 1;
                }
                vec![Action::Focus]
            }
            Input::Body { control, .. } => {
                if control || !self.view.body_button {
                    self.ignored_reason = if control {
                        "child-control"
                    } else {
                        "body-disabled"
                    };
                    return vec![];
                }
                vec![Action::Body {
                    id: record.id,
                    open_main: record.interaction.is_some() && record.message_kind == "tutorial",
                }]
            }
            Input::Key {
                key,
                composing,
                key_code,
                control,
                ..
            } => {
                if composing || key_code == 229 {
                    return vec![];
                }
                if key == "Escape" {
                    return self.input(Input::Dismiss { id: record.id }, now);
                }
                if control {
                    return vec![];
                }
                match key.as_str() {
                    "ArrowUp" => self.navigate(BubbleDeckDirection::Older),
                    "ArrowDown" => self.navigate(BubbleDeckDirection::Newer),
                    "End" => self.navigate(BubbleDeckDirection::Latest),
                    "Enter" | " " => self.input(
                        Input::Body {
                            id: record.id,
                            control: false,
                        },
                        now,
                    ),
                    _ => vec![],
                }
            }
            Input::Dismiss { id } => {
                if !self.view.dismissible || self.view.busy {
                    return vec![];
                }
                let token = self.begin(false);
                vec![Action::Dismiss { id, token }]
            }
            Input::Navigate { direction } => self.navigate(direction),
            Input::Wheel {
                delta,
                control,
                scroll_top,
                client_height,
                scroll_height,
            } => {
                if delta == 0.0
                    || control
                    || self.view.navigation_disabled
                    || (delta < 0.0 && scroll_top > 0.0)
                    || (delta > 0.0 && scroll_top + client_height < scroll_height - 1.0)
                    || self.last_wheel.is_some_and(|previous| {
                        now.duration_since(previous) < Duration::from_millis(200)
                    })
                {
                    return vec![];
                }
                self.last_wheel = Some(now);
                self.navigate(if delta < 0.0 {
                    BubbleDeckDirection::Older
                } else {
                    BubbleDeckDirection::Newer
                })
            }
            Input::Select { id, value } => {
                let Some(select) = record.interaction.and_then(|i| i.select) else {
                    return vec![];
                };
                if !select.options.iter().any(|o| o.value == value) || self.view.busy {
                    return vec![];
                }
                let token = self.begin(false);
                vec![Action::Interact {
                    id,
                    token,
                    action: select.action,
                    value: Some(value),
                }]
            }
            Input::Secret { id, value } => {
                let Some(secret) = record.interaction.and_then(|i| i.secret_input) else {
                    return vec![];
                };
                if self.view.busy {
                    return vec![];
                }
                if value.trim().is_empty() {
                    self.view.required_input = true;
                    return vec![];
                }
                let token = self.begin(true);
                vec![Action::Interact {
                    id,
                    token,
                    action: secret.action,
                    value: Some(value),
                }]
            }
            Input::Action { id, action } => {
                if self.view.busy
                    || record
                        .interaction
                        .is_none_or(|i| !i.actions.iter().any(|a| a.id == action))
                {
                    return vec![];
                }
                let token = self.begin(false);
                vec![Action::Interact {
                    id,
                    token,
                    action,
                    value: None,
                }]
            }
        }
    }

    fn navigate(&self, direction: BubbleDeckDirection) -> Vec<BubbleControlAction> {
        if self.view.navigation_disabled {
            return vec![];
        }
        vec![
            BubbleControlAction::Focus,
            BubbleControlAction::Navigate(direction),
        ]
    }

    fn begin(&mut self, secret: bool) -> u64 {
        self.operation += 1;
        self.secret_operation = secret;
        self.view.busy = true;
        self.view.error = None;
        self.view.required_input = false;
        self.operation
    }
}

