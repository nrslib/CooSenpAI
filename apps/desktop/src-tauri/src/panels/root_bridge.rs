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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "settings-root.integration.test.tsx から使う画面を起動しない stdio bridge"]
async fn settings_dom_bridge() {
    let (root, task) = crate::ui_root::test_channel(Port);
    println!("SETTINGS_ROOT {}", json!({"ready":true}));
    std::io::stdout().flush().unwrap();
    for line in std::io::stdin().lock().lines() {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let value = match request["operation"].as_str().unwrap() {
            "close" => break,
            "hide-details" => {
                root.request(
                    UiView::Application,
                    UiEvent::Window {
                        view: crate::ui_events::PresenterId::Details,
                        event: crate::presentation::PresentationEvent::Hide,
                    },
                )
                .await
                .unwrap();
                Value::Null
            }
            "open-details" => {
                root.request(UiView::Application, UiEvent::OpenDetails)
                    .await
                    .unwrap();
                Value::Null
            }
            "snapshot-updated" => {
                let snapshot: crate::snapshot::AppSnapshot =
                    serde_json::from_value(request["snapshot"].clone()).unwrap();
                root.request(
                    UiView::Application,
                    UiEvent::SnapshotUpdated(std::sync::Arc::new(snapshot)),
                )
                .await
                .unwrap();
                Value::Null
            }
            "snapshot" => serde_json::to_value(crate::snapshot::AppSnapshot::initial(
                Default::default(),
                vec![],
                coosenpai_core::ports::ScreenCapturePermission::from_preflight(false, false),
                Default::default(),
                0,
                0,
                false,
            ))
            .unwrap(),
            "config" => {
                let mut config =
                    serde_json::to_value(coosenpai_core::config::Config::default()).unwrap();
                config["revision"] = json!(1);
                config
            }
            "panel" => {
                let panel: PanelRequest =
                    serde_json::from_value(request["payload"].clone()).unwrap();
                let owner = panel.kind.owner(false);
                match root
                    .query(UiView::Application, |reply| UiEvent::Panel {
                        owner,
                        request: panel,
                        reply,
                    })
                    .await
                    .unwrap()
                {
                    Ok(output) => json!({"ok":true,"value":output}),
                    Err(message) => json!({"ok":false,"error":{"message":message}}),
                }
            }
            operation => panic!("unknown operation: {operation}"),
        };
        println!(
            "SETTINGS_ROOT {}",
            json!({"sequence":request["sequence"],"value":value})
        );
        std::io::stdout().flush().unwrap();
    }
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}
