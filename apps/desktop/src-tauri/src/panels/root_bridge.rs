use super::*;
use crate::ui_events::{EffectResult, UiEffect, UiEvent, UiTask, UiView};
use crate::ui_root::UiPort;
use serde_json::json;
use std::io::{BufRead, Write};

// ネイティブ実装を持たず、Root が返したパネル出力だけを配送する。
struct Port;
#[async_trait::async_trait]
impl UiPort for Port {
    async fn execute(&self, effect: UiEffect) -> Result<EffectResult, String> {
        match effect {
            UiEffect::PanelOutput { reply, output } => {
                let _ = reply.send(output);
            }
            UiEffect::PanelUpdates { updates, .. } => {
                println!("SETTINGS_ROOT {}", json!({"panelUpdates": updates}));
                std::io::stdout().flush().unwrap();
            }
            UiEffect::Log(_)
            | UiEffect::View { .. }
            | UiEffect::RenderWindow(_)
            | UiEffect::TrayRender(_)
            | UiEffect::AppRender(_)
            | UiEffect::StatusRender(_)
            | UiEffect::ComposerRender(_)
            | UiEffect::ConversationRender(_)
            | UiEffect::WorkApprovalRender(_)
            | UiEffect::AvatarRender(_)
            | UiEffect::BubbleRender { .. }
            | UiEffect::BubbleControls(_)
            | UiEffect::BubbleTyping { .. }
            | UiEffect::MainFocus(_) => {}
            _ => panic!("DOM bridge must not execute native effects"),
        }
        Ok(EffectResult::done())
    }
    async fn run(&self, task: UiTask) -> Result<EffectResult, String> {
        match task {
            UiTask::Load {
                view,
                generation,
                request,
            } => {
                let mut result = EffectResult::done();
                result.events.push(UiEvent::Window {
                    view,
                    event: crate::presentation::PresentationEvent::Loaded {
                        generation,
                        result: Ok(Some(crate::ui_load::test_content(request))),
                    },
                });
                Ok(result)
            }
            _ => panic!("DOM bridge must not execute native tasks"),
        }
    }
}
