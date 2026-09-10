use super::*;
use serde_json::json;

struct Screen {
    panels: PanelPresenters,
    kind: PanelKind,
    last: PanelOutput,
}
impl Screen {
    fn new(kind: PanelKind, value: Value) -> Self {
        let mut panels = PanelPresenters::default();
        let last = panels
            .handle(PanelRequest {
                session: "test".into(),
                kind,
                event: PanelEvent::Mount { value },
            })
            .unwrap();
        Self { panels, kind, last }
    }
    fn event(&mut self, event: PanelEvent) -> &PanelOutput {
        self.last = self
            .panels
            .handle(PanelRequest {
                session: "test".into(),
                kind: self.kind,
                event,
            })
            .unwrap();
        &self.last
    }
    fn action(&mut self, name: &str, value: Value) -> &PanelOutput {
        self.event(PanelEvent::Action {
            name: name.into(),
            value,
        })
    }
    fn command(&self, kind: &str) -> PanelCommand {
        self.last
            .commands
            .iter()
            .find(|c| c.kind == kind)
            .unwrap_or_else(|| panic!("missing {kind}: {:?}", self.last))
            .clone()
    }
    fn done(&mut self, command: &PanelCommand, value: Value) -> &PanelOutput {
        let value = if self.kind == PanelKind::Settings
            && matches!(
                command.kind.as_str(),
                "save" | "selectPersona" | "reloadConflict"
            ) {
            json!({"fields":form_fields(&value),"config":value})
        } else {
            value
        };
        self.event(PanelEvent::Completed {
            id: command.id,
            result: IoResult {
                ok: true,
                value,
                error: None,
                issues: None,
            },
        })
    }
    fn fail(&mut self, command: &PanelCommand, message: &str) -> &PanelOutput {
        self.event(PanelEvent::Completed {
            id: command.id,
            result: IoResult {
                ok: false,
                value: Value::Null,
                error: Some(IoError {
                    message: message.into(),
                    issues: None,
                }),
                issues: None,
            },
        })
    }
}
fn config() -> Value {
    json!({"revision":1,"ui":{"language":"ja","theme":"system","avatarPath":null},"companion":{"persona":"default","displayName":"Coo"},"watch":{"downscaleWidth":1280,"apps":[]}})
}
fn form_fields(config: &Value) -> Value {
    fn visit(value: &Value, path: Vec<String>, fields: &mut serde_json::Map<String, Value>) {
        if let Some(object) = value.as_object() {
            for (key, child) in object {
                if key == "revision" {
                    continue;
                }
                let mut path = path.clone();
                path.push(key.clone());
                visit(child, path, fields);
            }
        } else {
            let name = match path.join(".").as_str() {
                "companion.displayName" => "displayName".into(),
                "companion.persona" => "persona".into(),
                "ui.theme" => "uiTheme".into(),
                "watch.downscaleWidth" => "downscaleWidth".into(),
                "watch.apps" => "watchApps".into(),
                "ui.avatarPath" => "avatarPath".into(),
                other => other.to_owned(),
            };
            let patch = path
                .iter()
                .rev()
                .fold(value.clone(), |leaf, key| json!({key:leaf}));
            let input = if value.is_number() {
                json!(value.to_string())
            } else {
                value.clone()
            };
            fields.insert(name, json!({"value":input,"patch":patch}));
        }
    }
    let mut fields = serde_json::Map::new();
    visit(config, vec![], &mut fields);
    Value::Object(fields)
}
fn observation(config: Value) -> Value {
    json!({"configRevision":config["revision"],"fields":form_fields(&config),"config":config,"avatarImageLoadFailed":false,"issues":[],"onboarding":{"tutorialActive":false},"focusSection":null})
}
fn draft(config: Value, revision: u64) -> Value {
    json!({"fields":form_fields(&config),"revision":revision,"avatarImage":null,"avatarFileName":null,"basis":{"configRevision":1,"generation":0,"fields":form_fields(&self::config()),"avatarImage":null,"avatarFileName":null}})
}
fn settings() -> Screen {
    Screen::new(
        PanelKind::Settings,
        json!({"observation":observation(config()),"draft":draft(config(),0)}),
    )
}

#[test]
fn settings_dirty_external_adoption_save_diff_and_success_timer() {
    let mut s = settings();
    assert_eq!(s.last.state["dirty"], false);
    let mut edited = config();
    edited["companion"]["displayName"] = json!("new");
    s.event(PanelEvent::Change {
        value: draft(edited, 1),
    });
    assert_eq!(s.last.state["dirty"], true);
    let mut external = config();
    external["revision"] = json!(2);
    external["watch"]["downscaleWidth"] = json!(640);
    s.action("snapshot", observation(external));
    assert!(s.last.commands.is_empty());
    s.action("save", Value::Null);
    let save = s.command("save");
    assert_eq!(
        save.payload["patch"],
        json!({"companion":{"displayName":"new"}})
    );
    assert_eq!(save.payload["baseConfigRevision"], 1);
    s.action("save", Value::Null);
    assert!(s.last.commands.is_empty());
    s.action("close", Value::Null);
    assert!(s.last.commands.is_empty());
    let mut saved = config();
    saved["revision"] = json!(3);
    saved["companion"]["displayName"] = json!("new");
    s.done(&save, saved.clone());
    assert_eq!(s.last.state["dirty"], false);
    assert_eq!(s.last.state["saved"], true);
    assert_eq!(s.command("reflect").payload["config"], saved);
    let timer = s.command("savedDelay");
    assert_eq!(timer.payload, 1500);
    s.done(&timer, Value::Null);
    assert_eq!(s.last.state["saved"], false);
    s.action("snapshot", observation(config()));
    assert!(s.last.commands.is_empty());
}
#[test]
fn settings_save_preserves_edits_made_during_save_and_stale_timer() {
    let mut s = settings();
    let mut edit = config();
    edit["companion"]["displayName"] = json!("first");
    s.event(PanelEvent::Change {
        value: draft(edit.clone(), 1),
    });
    s.action("save", Value::Null);
    let save = s.command("save");
    edit["companion"]["displayName"] = json!("second");
    s.event(PanelEvent::Change {
        value: draft(edit, 2),
    });
    let mut result = config();
    result["revision"] = json!(2);
    result["companion"]["displayName"] = json!("first");
    s.done(&save, result);
    assert_eq!(s.last.state["dirty"], true);
    assert_eq!(s.last.state["saved"], false);
    assert_eq!(
        s.command("reflect").payload["config"]["companion"]["displayName"],
        "second"
    );
    assert_eq!(s.command("reflect").payload["revision"], 2);
    s.done(&save, config());
    assert!(s.last.commands.is_empty());
}
#[test]
fn settings_conflict_reloads_and_rebases_arrays_and_avatar_without_resaving() {
    let mut s = settings();
    let mut local = config();
    local["companion"]["displayName"] = json!("local");
    local["watch"]["apps"] = json!([{"bundleId":"local.app"}]);
    let mut input = draft(local, 1);
    input["avatarImage"] = json!([1, 2, 3]);
    input["avatarFileName"] = json!("avatar.png");
    s.event(PanelEvent::Change { value: input });
    s.action("save", Value::Null);
    let save = s.command("save");
    assert_eq!(
        save.payload["patch"]["ui"]["avatarPath"],
        "state/avatar.png"
    );
    s.fail(&save, "設定が別の場所で変更されました。読み直してください");
    let reload = s.command("reloadConflict");
    let mut latest = config();
    latest["revision"] = json!(2);
    latest["watch"]["downscaleWidth"] = json!(640);
    latest["companion"]["displayName"] = json!("external");
    s.done(&reload, latest);
    assert_eq!(s.last.state["dirty"], true);
    assert_eq!(s.last.state["saving"], false);
    let reflected = s.command("reflect");
    assert_eq!(reflected.payload["config"]["watch"]["downscaleWidth"], 640);
    assert_eq!(
        reflected.payload["config"]["companion"]["displayName"],
        "local"
    );
    assert_eq!(reflected.payload["avatarImage"], json!([1, 2, 3]));
    assert!(!s.last.commands.iter().any(|c| c.kind == "save"));
    assert_eq!(
        s.last.state["externalChanges"],
        json!(["companion.displayName", "watch.downscaleWidth"])
    );
    s.action("save", Value::Null);
    assert_eq!(s.command("save").payload["baseConfigRevision"], 2);
}
#[test]
fn settings_close_requires_clear_success_and_escape_prioritizes_recording() {
    let mut s = settings();
    s.action("openVrm", Value::Null);
    s.action("recording", json!("capture"));
    let esc = json!({"key":"Escape","composing":false,"keyCode":27});
    s.action(
        "escape",
        json!({"key":"Escape","composing":true,"keyCode":229}),
    );
    assert_eq!(s.last.state["recordingShortcut"], "capture");
    s.action("escape", esc.clone());
    assert!(s.last.state["recordingShortcut"].is_null());
    assert_eq!(s.last.state["vrmControlsOpen"], true);
    s.action("escape", esc.clone());
    assert_eq!(s.last.state["vrmControlsOpen"], false);
    assert!(s.last.commands.is_empty());
    let mut local = config();
    local["ui"]["theme"] = json!("dark");
    s.event(PanelEvent::Change {
        value: draft(local, 1),
    });
    s.action("escape", esc);
    assert_eq!(s.last.state["discardConfirmOpen"], true);
    s.action("discard", Value::Null);
    let clear = s.command("clearPreview");
    assert!(!s.last.commands.iter().any(|c| c.kind == "close"));
    s.fail(&clear, "clear failed");
    assert_eq!(s.last.state["issues"][0]["message"], "clear failed");
    s.action("discard", Value::Null);
    let clear = s.command("clearPreview");
    s.done(&clear, Value::Null);
    s.command("close");
}
#[test]
fn settings_tutorial_focus_persona_selection_confirmation_and_errors() {
    let mut s = settings();
    let mut observed = observation(config());
    observed["onboarding"] =
        json!({"tutorialActive":true,"currentStep":"persona","settingsHighlight":"persona"});
    s.action("snapshot", observed.clone());
    assert_eq!(s.last.state["personaPickerOpen"], true);
    assert_eq!(s.last.state["activeCategory"], "general");
    s.action("category", json!("vision"));
    s.action("snapshot", observed.clone());
    assert_eq!(s.last.state["activeCategory"], "vision");
    observed["onboarding"] =
        json!({"tutorialActive":true,"currentStep":"watch","settingsHighlight":"watch"});
    s.action("snapshot", observed);
    assert_eq!(s.last.state["personaPickerOpen"], false);
    s.action("openPicker", Value::Null);
    assert_eq!(s.last.state["personaPickerOpen"], false);
    s.action("focusIssue", json!("voiceOutput.provider"));
    assert_eq!(s.last.state["activeCategory"], "beta");
    s.command("focusIssue");
    s.action("confirm", json!("tuning"));
    s.action("acceptConfirmation", Value::Null);
    s.command("resetTuning");
    assert!(s.last.state["confirmation"].is_null());
    s.action("confirm", json!("conversation-reset"));
    s.action("acceptConfirmation", Value::Null);
    s.command("resetConversation");
    s.action("editPersona", json!("default"));
    let old = s.command("loadPersona");
    s.action("closePersona", Value::Null);
    s.done(&old, json!({"id":"default"}));
    assert!(s.last.state["personaDocument"].is_null());
    s.action("editPersona", json!("default"));
    let load = s.command("loadPersona");
    s.fail(&load, "read failed");
    assert_eq!(s.last.state["issues"][0]["path"], "companion.persona");
    s.action("selectPersona", json!("new"));
    let select = s.command("selectPersona");
    let mut selected = config();
    selected["revision"] = json!(2);
    selected["companion"]["persona"] = json!("new");
    s.done(&select, selected);
    assert_eq!(
        s.command("reflect").payload["config"]["companion"]["persona"],
        "new"
    );
}
#[test]
fn persona_save_delete_restore_wait_for_reflection_before_closing_and_fail_open() {
    for operation in ["save", "delete", "restore"] {
        let mut s = Screen::new(
            PanelKind::Persona,
            json!({"builtin":false,"originalId":"custom","draft":{"id":"custom","displayName":"Name","body":"Body"}}),
        );
        if operation == "delete" {
            s.action("requestDelete", Value::Null);
            assert_eq!(s.last.state["deleteConfirmOpen"], true);
        }
        s.action(
            operation,
            if operation == "restore" {
                json!("version-1")
            } else {
                Value::Null
            },
        );
        let task = s.command(operation);
        assert_eq!(s.last.state["busy"], true);
        s.action("save", Value::Null);
        assert!(s.last.commands.is_empty());
        s.fail(&task, "failed");
        assert_eq!(s.last.state["busy"], false);
        assert!(s.last.commands.is_empty());
        if operation == "delete" {
            s.action("requestDelete", Value::Null);
        }
        s.action(
            operation,
            if operation == "restore" {
                json!("version-1")
            } else {
                Value::Null
            },
        );
        let task = s.command(operation);
        s.done(&task, Value::Null);
        let refresh = s.command("refresh");
        assert!(!s.last.commands.iter().any(|c| c.kind == "close"));
        s.done(&refresh, Value::Null);
        s.command("close");
    }
    let mut s = Screen::new(
        PanelKind::Persona,
        json!({"builtin":true,"originalId":"builtin","draft":{"id":"../bad","displayName":"Name","body":"Body"}}),
    );
    assert_eq!(s.last.state["canSave"], false);
    s.action("requestDelete", Value::Null);
    assert_eq!(s.last.state["deleteConfirmOpen"], false);
}
#[test]
fn attachment_stale_load_expiry_and_expansion_are_presenter_decisions() {
    let mut s = Screen::new(PanelKind::Attachment, json!("old.png"));
    let old = s.command("read");
    s.action("expand", Value::Null);
    assert_eq!(s.last.state["expanded"], false);
    s.event(PanelEvent::Change {
        value: json!("new.png"),
    });
    let new = s.command("read");
    s.done(&old, json!("blob:old"));
    assert!(s.last.state["source"].is_null());
    s.fail(&new, "missing");
    assert_eq!(s.last.state["expired"], true);
    s.event(PanelEvent::Change {
        value: json!("third.png"),
    });
    let third = s.command("read");
    s.done(&third, json!("blob:third"));
    s.action("expand", Value::Null);
    assert_eq!(s.last.state["expanded"], true);
    s.action("close", Value::Null);
    assert_eq!(s.last.state["expanded"], false);
    s.event(PanelEvent::Unmount);
    s.done(&old, json!("blob:late"));
    assert!(s.last.state.is_null());
}
#[test]
fn update_initial_observation_operation_and_deferred_install() {
    let mut s = Screen::new(
        PanelKind::Update,
        json!({"enabled":true,"showControls":false}),
    );
    let load = s.command("load");
    s.action(
        "observed",
        json!({"revision":10,"status":{"phase":"available","version":"2"}}),
    );
    s.done(&load, json!({"revision":1,"status":{"phase":"idle"}}));
    assert_eq!(s.last.state["snapshot"]["revision"], 10);
    s.action("install", Value::Null);
    let install = s.command("install");
    assert_eq!(s.last.state["canCheck"], false);
    s.action("install", Value::Null);
    assert!(s.last.commands.is_empty());
    s.action(
        "observed",
        json!({"revision":12,"status":{"phase":"installed","version":"2"}}),
    );
    s.done(
        &install,
        json!({"revision":11,"status":{"phase":"downloading","version":"2"}}),
    );
    assert_eq!(s.last.state["canRestart"], true);
    assert_eq!(s.last.state["visible"], true);
    s.action("later", Value::Null);
    assert_eq!(s.last.state["visible"], false);
    s.event(PanelEvent::Change {
        value: json!({"enabled":false,"showControls":true}),
    });
    assert_eq!(s.last.state["visible"], true);
    assert_eq!(s.last.state["canRestart"], true);
}
#[test]
fn voice_latest_operation_controls_error_and_stop_can_interrupt_test() {
    let mut s = Screen::new(
        PanelKind::Voice,
        json!({"enabled":true,"showControls":true}),
    );
    let load = s.command("load");
    assert_eq!(s.last.state["canTest"], false);
    s.done(&load, json!({"revision":1,"speaking":false,"message":null}));
    s.action("test", Value::Null);
    let test = s.command("test");
    assert_eq!(s.last.state["canTest"], false);
    assert_eq!(s.last.state["canStop"], true);
    s.action("stop", Value::Null);
    let stop = s.command("stop");
    s.done(&stop, json!({"revision":4,"speaking":false,"message":null}));
    assert_eq!(s.last.state["snapshot"]["revision"], 4);
    s.fail(&test, "old failure");
    assert!(s.last.state["error"].is_null());
    assert_eq!(s.last.state["busy"], false);
    s.action(
        "observed",
        json!({"revision":3,"speaking":true,"message":null}),
    );
    assert_eq!(s.last.state["snapshot"]["speaking"], false);
    s.event(PanelEvent::Change {
        value: json!({"enabled":false,"showControls":true}),
    });
    assert_eq!(s.last.state["canTest"], false);
}
#[test]
fn vrm_verify_save_notify_show_and_remove_notify_hide_each_wait_for_completion() {
    let mut s = Screen::new(PanelKind::Vrm, Value::Null);
    let load = s.command("load");
    s.done(&load, json!({"name":null,"quality":"medium"}));
    s.action("select", json!({"name":"file.vrm","fileId":"file-1"}));
    let verify = s.command("verify");
    assert_eq!(verify.payload["fileId"], "file-1");
    s.action("select", json!({"name":"other.vrm","fileId":"file-2"}));
    assert!(s.last.commands.is_empty());
    s.fail(&verify, "invalid");
    assert_eq!(s.last.state["busy"], false);
    assert_eq!(s.last.state["error"], "invalid");
    s.command("release");
    s.action("select", json!({"name":"file.vrm","fileId":"file-1"}));
    let verify = s.command("verify");
    s.done(&verify, Value::Null);
    s.command("save");
    let save = s.command("save");
    s.done(&save, Value::Null);
    let notify = s.command("notify");
    assert_eq!(s.last.state["modelName"], "file.vrm");
    assert_eq!(s.last.state["busy"], true);
    s.done(&notify, Value::Null);
    let show = s.command("show");
    s.done(&show, Value::Null);
    assert_eq!(s.last.state["busy"], false);
    s.command("release");
    s.action("remove", Value::Null);
    let remove = s.command("remove");
    s.done(&remove, Value::Null);
    let notify = s.command("notify");
    assert!(s.last.state["modelName"].is_null());
    s.done(&notify, Value::Null);
    let hide = s.command("hide");
    s.done(&hide, Value::Null);
    assert_eq!(s.last.state["busy"], false);
}
fn snapshot(revision: u64) -> Value {
    json!({"revision":revision,"observer":{},"audio":{"generation":1,"phase":"off","recentEvents":[]},"conversation":[],"conversationGenerations":[{"generation":1},{"generation":2}],"selectedConversationGeneration":1})
}
#[test]
fn details_tabs_debug_selection_and_history_acceptance_are_independent() {
    let mut s = Screen::new(PanelKind::Details, Value::Null);
    s.command("ready");
    s.action(
        "history",
        json!({"ok":true,"value":{"observations":[],"transcripts":[]}}),
    );
    s.action("snapshot", snapshot(1));
    s.action("tab", json!("conversation"));
    s.action("debug", json!({"title":"trace"}));
    s.action("select", json!(1));
    assert!(s.last.commands.is_empty());
    s.action("select", json!(2));
    let select = s.command("select");
    assert_eq!(s.last.state["switching"], true);
    s.action("select", json!(2));
    assert!(s.last.commands.is_empty());
    let mut fresh = snapshot(3);
    fresh["selectedConversationGeneration"] = json!(2);
    s.action("snapshot", fresh);
    s.done(&select, snapshot(2));
    assert_eq!(s.last.state["snapshot"]["revision"], 3);
    assert_eq!(s.last.state["switching"], false);
    assert_eq!(s.last.state["activeTab"], "conversation");
    assert_eq!(s.last.state["debugDetail"]["title"], "trace");
    s.action("debug", Value::Null);
    assert!(s.last.state["debugDetail"].is_null());
}
#[test]
fn dataflow_preserves_live_events_when_history_arrives_and_bounds_deduplicated_events() {
    let mut s = Screen::new(PanelKind::Details, Value::Null);
    s.action("snapshot", snapshot(1));
    for revision in 2..=205 {
        let mut next = snapshot(revision);
        next["latestCompanionDecision"] = json!({"sequence":revision,"occurredAt":format!("2026-09-08T00:{:02}:{:02}.000Z",revision/60,revision%60),"emit":false,"thought":"wait"});
        s.action("snapshot", next);
    }
    s.action(
        "history",
        json!({"ok":true,"value":{"observations":[],"transcripts":[]}}),
    );
    let events = s.last.state["dataFlow"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 200);
    assert_eq!(events.last().unwrap()["id"], "companion-decision:205");
    let saved = events.clone();
    s.action("snapshot", snapshot(1));
    assert_eq!(s.last.state["dataFlow"]["events"], json!(saved));
    s.action(
        "history",
        json!({"ok":false,"error":{"message":"late failure"}}),
    );
    assert!(s.last.state["dataFlow"]["loadError"].is_null());
}
#[test]
fn dataflow_hearing_interruption_and_first_history_use_recorded_times() {
    let mut s = Screen::new(PanelKind::Details, Value::Null);
    s.action("snapshot", snapshot(1));
    let mut next = snapshot(2);
    next["audio"]["phase"] = json!("listening");
    next["audio"]["recentEvents"] = json!([{"id":"final","stage":"confirmed","createdAt":"2026-09-08T01:00:00Z","text":"hello","source":"microphone"}]);
    next["latestUserInterruption"] =
        json!({"sequence":4,"occurredAt":"2026-09-08T01:00:01Z","observer":true,"proactive":true});
    s.action("snapshot", next);
    let events = s.last.state["dataFlow"]["events"].as_array().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events.iter().find(|e| e["id"] == "hearing:final").unwrap()["at"],
        "2026-09-08T01:00:00Z"
    );
    s.action("history",json!({"ok":true,"value":{"observations":[{"kind":"audio","id":"old","createdAt":"2026-09-07T00:00:00Z","text":"old","source":"speaker"}],"transcripts":[{"observationId":"old","text":"canonical"}]}}));
    assert_eq!(
        s.last.state["dataFlow"]["events"][0]["content"]["data"]["transcript"],
        "canonical"
    );
}
#[test]
fn dataflow_filters_expansion_and_scroll_threshold() {
    let events = json!([{"id":"1","kind":"visual","summary":"Editor","detail":"saved"},{"id":"2","kind":"hearing","summary":"Speech","detail":"Meeting"}]);
    let mut s = Screen::new(PanelKind::Dataflow, events.clone());
    s.command("scrollEnd");
    s.action("filter", json!("hearing"));
    assert_eq!(s.last.state["visible"].as_array().unwrap().len(), 1);
    s.action("query", json!(" editor "));
    assert_eq!(s.last.state["visible"], json!([]));
    s.action("query", json!(" meeting "));
    assert_eq!(s.last.state["visible"][0]["id"], "2");
    s.action("toggle", json!("2"));
    assert_eq!(s.last.state["expanded"], json!(["2"]));
    s.action("scroll", json!(24));
    s.event(PanelEvent::Change {
        value: events.clone(),
    });
    assert!(s.last.commands.is_empty());
    s.action("scroll", json!(23));
    s.event(PanelEvent::Change { value: events });
    s.command("scrollEnd");
    let mut history = Screen::new(PanelKind::History, json!([]));
    history.action("filter", json!("image"));
    assert_eq!(history.last.state["filter"], "image");
    history.action("expand", json!("full text"));
    assert_eq!(history.last.state["expandedText"], "full text");
    history.action("close", Value::Null);
    assert!(history.last.state["expandedText"].is_null());
}

#[test]
fn vrm_cancel_before_success_never_starts_save_and_quality_waits_for_notify() {
    let mut s = Screen::new(PanelKind::Vrm, Value::Null);
    let load = s.command("load");
    s.done(&load, json!({"name":null,"quality":"medium"}));
    s.action("select", json!({"fileId":"1","name":"model.vrm"}));
    let verify = s.command("verify");
    s.action("cancel", Value::Null);
    s.command("abort");
    s.done(&verify, Value::Null);
    assert_eq!(s.last.state["aborted"], true);
    assert_eq!(s.last.state["busy"], false);
    assert!(!s.last.commands.iter().any(|c| c.kind == "save"));
    s.action("quality", json!("high"));
    let save = s.command("saveQuality");
    assert_eq!(s.last.state["imageQuality"], "medium");
    s.done(&save, Value::Null);
    let notify = s.command("notify");
    assert_eq!(s.last.state["imageQuality"], "high");
    assert_eq!(s.last.state["busy"], true);
    s.fail(&notify, "notify failed");
    assert_eq!(s.last.state["error"], "notify failed");
    assert_eq!(s.last.state["busy"], false);
}
#[test]
fn settings_clean_snapshot_adopts_form_and_failed_rebase_keeps_local_draft() {
    let mut s = settings();
    let mut external = config();
    external["revision"] = json!(2);
    external["ui"]["theme"] = json!("dark");
    s.action("snapshot", observation(external.clone()));
    assert_eq!(
        s.command("reflect").payload["config"]["ui"]["theme"],
        "dark"
    );
    external["companion"]["displayName"] = json!("local");
    s.event(PanelEvent::Change {
        value: draft(external, 1),
    });
    s.action("save", Value::Null);
    let save = s.command("save");
    s.fail(
        &save,
        "The configuration changed elsewhere. Reload it and try again.",
    );
    let reload = s.command("reloadConflict");
    s.fail(&reload, "disk failed");
    assert_eq!(s.last.state["saving"], false);
    assert_eq!(s.last.state["dirty"], true);
    assert!(s.last.state["issues"][0]["message"]
        .as_str()
        .unwrap()
        .contains("disk failed"));
    s.action("save", Value::Null);
    assert_eq!(
        s.command("save").payload["patch"],
        json!({"companion":{"displayName":"local"}})
    );
}
#[test]
fn initial_status_failure_does_not_overwrite_notification_or_newer_operation_error() {
    for kind in [PanelKind::Update, PanelKind::Voice] {
        let mut s = Screen::new(kind, json!({"enabled":true,"showControls":true}));
        let load = s.command("load");
        s.action(
            "observed",
            if kind == PanelKind::Update {
                json!({"revision":1,"status":{"phase":"upToDate"}})
            } else {
                json!({"revision":1,"speaking":false,"message":null})
            },
        );
        s.fail(&load, "old load failed");
        assert!(s.last.state["error"].is_null());
        s.action(
            if kind == PanelKind::Update {
                "check"
            } else {
                "test"
            },
            Value::Null,
        );
        let command = s.command(if kind == PanelKind::Update {
            "check"
        } else {
            "test"
        });
        s.fail(&command, "current failure");
        assert_eq!(s.last.state["error"], "current failure");
    }
}

#[test]
fn incompatible_update_is_visible_and_cannot_install_or_restart() {
    for show_controls in [false, true] {
        let mut screen = Screen::new(
            PanelKind::Update,
            json!({"enabled":true,"showControls":show_controls}),
        );
        let load = screen.command("load");
        screen.done(&load, json!({"revision":1,"status":{"phase":"incompatible","version":"1.1.0","minimumSystemVersion":"26.0"}}));
        assert_eq!(screen.last.state["visible"], true);
        assert_eq!(screen.last.state["canCheck"], true);
        assert_eq!(screen.last.state["canInstall"], false);
        assert_eq!(screen.last.state["showInstall"], false);
        assert_eq!(screen.last.state["canRestart"], false);
        assert_eq!(screen.last.state["showRestart"], false);
        assert!(screen.last.state["error"].is_null());
        screen.action("install", Value::Null);
        assert!(screen.last.commands.is_empty());
        screen.action("restart", Value::Null);
        assert!(screen.last.commands.is_empty());
    }
}
#[test]
fn persona_ime_escape_and_delete_confirmation_priority() {
    let mut s = Screen::new(
        PanelKind::Persona,
        json!({"builtin":false,"originalId":"custom","draft":{"id":"custom","displayName":"Name","body":"Body"}}),
    );
    s.action("requestDelete", Value::Null);
    s.action(
        "key",
        json!({"key":"Escape","composing":true,"keyCode":229}),
    );
    assert_eq!(s.last.state["deleteConfirmOpen"], true);
    s.action(
        "key",
        json!({"key":"Escape","composing":false,"keyCode":27}),
    );
    assert_eq!(s.last.state["deleteConfirmOpen"], false);
    assert!(s.last.commands.is_empty());
    s.action(
        "key",
        json!({"key":"Escape","composing":false,"keyCode":27}),
    );
    s.command("close");
}
#[test]
fn category_mapping_preserves_path_families() {
    for (path, category) in [
        ("work.allowedRoots[0].path", "work"),
        ("observer.provider", "providers"),
        ("observer.textExcerptMaxChars", "vision"),
        ("speech.locale", "general"),
        ("retention.observationDays", "vision"),
        ("audio.enabled", "hearing"),
        ("voiceOutput.rate", "beta"),
        ("companion.emotionsEnabled", "general"),
        ("companion.proactiveQuietMinutes", "vision"),
        ("companion.timeoutMs", "providers"),
        ("ui.thoughtBubble", "notifications"),
        ("speech.inputDevice", "speech"),
        ("keymap.sendKey", "shortcuts"),
    ] {
        let mut s = settings();
        s.action("focusIssue", json!(path));
        assert_eq!(s.last.state["activeCategory"], category, "{path}");
    }
}

#[tokio::test]
async fn root_routes_panel_input_and_completion_to_each_registered_parent() {
    use crate::ui_events::{EffectResult, UiEffect, UiEvent, UiTask, UiView};
    use crate::ui_root::UiPort;
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Port(Arc<Mutex<Vec<String>>>);
    #[async_trait::async_trait]
    impl UiPort for Port {
        async fn execute(&self, effect: UiEffect) -> Result<EffectResult, String> {
            match effect {
                UiEffect::PanelOutput { reply, output } => {
                    let _ = reply.send(output);
                }
                UiEffect::Log(message) => self.0.lock().unwrap().push(message),
                _ => panic!("unexpected effect"),
            }
            Ok(EffectResult::done())
        }
        async fn run(&self, _task: UiTask) -> Result<EffectResult, String> {
            panic!("browser I/O is returned as a command")
        }
    }
    let port = Port::default();
    let (root, task) = crate::ui_root::test_channel(port.clone());
    for (kind, owner, value) in [
        (
            PanelKind::Settings,
            PresenterId::Settings,
            json!({"observation":observation(config()),"draft":draft(config(),0)}),
        ),
        (PanelKind::Details, PresenterId::Details, Value::Null),
        (
            PanelKind::Voice,
            PresenterId::Chat,
            json!({"enabled":true,"showControls":true}),
        ),
    ] {
        let result = root
            .query(UiView::Application, |reply| UiEvent::Panel {
                owner,
                request: PanelRequest {
                    session: format!("{kind:?}"),
                    kind,
                    event: PanelEvent::Mount { value },
                },
                reply,
            })
            .await
            .unwrap()
            .unwrap();
        assert!(!result.state.is_null());
        if kind == PanelKind::Voice {
            let load = result
                .commands
                .iter()
                .find(|c| c.kind == "load")
                .unwrap()
                .id;
            let output = root
                .query(UiView::Application, |reply| UiEvent::Panel {
                    owner,
                    request: PanelRequest {
                        session: format!("{kind:?}"),
                        kind,
                        event: PanelEvent::Completed {
                            id: load,
                            result: IoResult {
                                ok: true,
                                value: json!({"revision":7,"speaking":false,"message":null}),
                                error: None,
                                issues: None,
                            },
                        },
                    },
                    reply,
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(output.state["canTest"], true);
        }
        assert!(port
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(&format!(
                "presenter={owner:?} event=Panel({owner:?}) handled"
            ))));
    }
    task.abort();
}

#[test]
fn malformed_history_failure_is_rejected_without_poisoning_the_session() {
    let mut s = Screen::new(PanelKind::Details, Value::Null);
    s.action("snapshot", snapshot(1));
    let result = s.panels.handle(PanelRequest {
        session: "test".into(),
        kind: PanelKind::Details,
        event: PanelEvent::Action {
            name: "history".into(),
            value: json!({"ok":false}),
        },
    });
    assert!(result.is_err());
    s.action(
        "history",
        json!({"ok":true,"value":{"observations":[],"transcripts":[]}}),
    );
    assert!(s.last.state["dataFlow"]["loadError"].is_null());
}

#[test]
fn settings_save_and_close_accept_the_latest_synchronous_form_edit() {
    let mut s = settings();
    let mut local = config();
    local["ui"]["theme"] = json!("dark");
    s.action("close", draft(local.clone(), 1));
    assert_eq!(s.last.state["discardConfirmOpen"], true);
    s.action("cancelDiscard", Value::Null);
    local["companion"]["displayName"] = json!("latest");
    s.action("save", draft(local, 2));
    assert_eq!(
        s.command("save").payload["patch"],
        json!({"ui":{"theme":"dark"},"companion":{"displayName":"latest"}})
    );
    s.event(PanelEvent::Change {
        value: draft(config(), 1),
    });
    assert_eq!(s.last.state["dirty"], true);
}

#[test]
fn attachment_history_filters_presenter_rows_and_keeps_filter_after_update() {
    let mut s = Screen::new(
        PanelKind::History,
        json!([{"kind":"image","input":{"id":"i"}},{"kind":"text","input":{"id":"t"}}]),
    );
    assert_eq!(s.last.state["visible"].as_array().unwrap().len(), 2);
    s.action("filter", json!("image"));
    assert_eq!(s.last.state["visible"][0]["input"]["id"], "i");
    assert_eq!(s.last.state["visible"].as_array().unwrap().len(), 1);
    s.event(PanelEvent::Change {
        value: json!([{"kind":"text","input":{"id":"t2"}}]),
    });
    assert_eq!(s.last.state["visible"], json!([]));
    assert_eq!(s.last.state["empty"], false);
}

fn child_event(s: &mut Screen, kind: PanelKind, event: PanelEvent) -> PanelOutput {
    s.panels
        .handle(PanelRequest {
            session: "child".into(),
            kind,
            event,
        })
        .unwrap()
}
fn child_action(s: &mut Screen, kind: PanelKind, name: &str, value: Value) -> PanelOutput {
    child_event(
        s,
        kind,
        PanelEvent::Action {
            name: name.into(),
            value,
        },
    )
}
fn child_done(
    s: &mut Screen,
    kind: PanelKind,
    command: &PanelCommand,
    value: Value,
) -> PanelOutput {
    child_event(
        s,
        kind,
        PanelEvent::Completed {
            id: command.id,
            result: IoResult {
                ok: true,
                value,
                error: None,
                issues: None,
            },
        },
    )
}
fn assert_parent_stays_open(s: &mut Screen, child_close: &str, visible: &str) {
    for (name, value) in [
        (child_close, Value::Null),
        (
            "escape",
            json!({"key":"Escape","composing":false,"keyCode":27}),
        ),
        ("close", Value::Null),
        ("discard", Value::Null),
    ] {
        s.action(name, value);
        assert!(
            !s.last.state[visible].is_null() && s.last.state[visible] != false,
            "{name}"
        );
        assert!(
            !s.last
                .commands
                .iter()
                .any(|c| matches!(c.kind.as_str(), "clearPreview" | "close")),
            "{name}"
        );
        assert_eq!(s.last.state["escapeEnabled"], false);
    }
}

#[test]
fn persona_operation_and_refresh_block_child_and_parent_closure() {
    for operation in ["save", "delete", "restore"] {
        let mut s = settings();
        s.action("editPersona", json!("custom"));
        let load = s.command("loadPersona");
        s.done(&load, json!({"id":"custom"}));
        let kind = PanelKind::Persona;
        child_event(
            &mut s,
            kind,
            PanelEvent::Mount {
                value: json!({"builtin":false,"originalId":"custom","draft":{"id":"custom","displayName":"Name","body":"Body"}}),
            },
        );
        if operation == "delete" {
            child_action(&mut s, kind, "requestDelete", Value::Null);
        }
        let started = child_action(
            &mut s,
            kind,
            operation,
            if operation == "restore" {
                json!("version")
            } else {
                Value::Null
            },
        );
        let task = started.commands[0].clone();
        for command in [task.kind.as_str(), "refresh"] {
            let close = child_action(&mut s, kind, "close", Value::Null);
            assert_eq!(close.state["busy"], true);
            assert!(close.commands.is_empty());
            let escape = child_action(
                &mut s,
                kind,
                "key",
                json!({"key":"Escape","composing":false,"keyCode":27}),
            );
            assert!(escape.commands.is_empty());
            assert_parent_stays_open(&mut s, "closePersona", "personaDocument");
            if command != "refresh" {
                let result = child_done(&mut s, kind, &task, Value::Null);
                assert_eq!(result.commands[0].kind, "refresh");
                // The refresh remains pending while the next iteration exercises every close route.
            }
        }
        let refresh_id = s.panels.sessions["child"]
            .io
            .pending
            .values()
            .find(|c| c.kind == "refresh")
            .unwrap()
            .clone();
        let refreshed = child_done(&mut s, kind, &refresh_id, Value::Null);
        assert_eq!(refreshed.commands[0].kind, "close");
        s.action("closePersona", Value::Null);
        assert!(s.last.state["personaDocument"].is_null());
        s.action("close", Value::Null);
        s.command("clearPreview");
    }
}

#[test]
fn vrm_each_io_stage_blocks_child_and_parent_closure() {
    for operation in ["select", "remove", "quality"] {
        let mut s = settings();
        s.action("openVrm", Value::Null);
        let kind = PanelKind::Vrm;
        let initial = child_event(&mut s, kind, PanelEvent::Mount { value: Value::Null });
        assert_parent_stays_open(&mut s, "closeVrm", "vrmControlsOpen");
        child_done(
            &mut s,
            kind,
            &initial.commands[0],
            json!({"name":"old.vrm","quality":"medium"}),
        );
        let mut output = child_action(
            &mut s,
            kind,
            operation,
            match operation {
                "select" => json!({"fileId":"file","name":"new.vrm"}),
                "quality" => json!("high"),
                _ => Value::Null,
            },
        );
        let mut stages = Vec::new();
        while output.state["busy"] == true {
            assert_parent_stays_open(&mut s, "closeVrm", "vrmControlsOpen");
            let task = output.commands[0].clone();
            stages.push(task.kind.clone());
            output = child_done(&mut s, kind, &task, Value::Null);
        }
        assert_eq!(
            stages,
            match operation {
                "select" => vec!["verify", "save", "notify", "show"],
                "remove" => vec!["remove", "notify", "hide"],
                _ => vec!["saveQuality", "notify"],
            }
        );
        s.action("closeVrm", Value::Null);
        assert_eq!(s.last.state["vrmControlsOpen"], false);
        s.action("close", Value::Null);
        s.command("clearPreview");
    }
}

#[test]
fn status_initial_load_blocks_check_even_after_notification_until_completion() {
    for failed in [false, true] {
        let mut s = Screen::new(
            PanelKind::Update,
            json!({"enabled":true,"showControls":true}),
        );
        let load = s.command("load");
        assert_eq!(s.last.state["busy"], true);
        assert_eq!(s.last.state["canCheck"], false);
        s.action("check", Value::Null);
        assert!(s.last.commands.is_empty());
        s.action("observed", json!({"revision":2,"status":{"phase":"idle"}}));
        assert_eq!(s.last.state["busy"], true);
        assert_eq!(s.last.state["canCheck"], false);
        if failed {
            s.fail(&load, "load failed");
        } else {
            s.done(&load, json!({"revision":1,"status":{"phase":"idle"}}));
        }
        assert_eq!(s.last.state["busy"], false);
        assert_eq!(s.last.state["canCheck"], true);
        assert_eq!(s.last.state["snapshot"]["revision"], 2);
        s.action("check", Value::Null);
        s.command("check");
    }
    let mut s = Screen::new(
        PanelKind::Update,
        json!({"enabled":true,"showControls":true}),
    );
    let load = s.command("load");
    s.fail(&load, "load failed");
    assert_eq!(s.last.state["error"], "load failed");
    assert_eq!(s.last.state["canCheck"], true);
}

#[test]
fn settings_closing_rejects_new_child_operations() {
    let mut s = settings();
    child_event(
        &mut s,
        PanelKind::Persona,
        PanelEvent::Mount {
            value: json!({"builtin":false,"originalId":"custom","draft":{"id":"custom","displayName":"Name","body":"Body"}}),
        },
    );
    s.action("close", Value::Null);
    s.command("clearPreview");
    s.action("openVrm", Value::Null);
    assert_eq!(s.last.state["vrmControlsOpen"], false);
    assert!(s.last.commands.is_empty());
    s.action("editPersona", json!("another"));
    assert!(s.last.commands.is_empty());
    let result = s.panels.handle(PanelRequest {
        session: "child".into(),
        kind: PanelKind::Persona,
        event: PanelEvent::Action {
            name: "save".into(),
            value: Value::Null,
        },
    });
    assert!(result.is_err());
}

#[test]
fn window_hide_waits_for_child_operations() {
    use crate::ui_events::{Handling, UiEffect, UiEvent};
    fn input(
        window: &mut crate::ui_presenters::WindowPresenter,
        kind: PanelKind,
        event: PanelEvent,
    ) -> PanelOutput {
        let (reply, _received) = tokio::sync::oneshot::channel();
        let Handling::Handled(effects) = window.handle(UiEvent::Panel {
            owner: PresenterId::Settings,
            request: PanelRequest {
                session: "child".into(),
                kind,
                event,
            },
            reply,
        }) else {
            panic!("panel must be handled");
        };
        let UiEffect::PanelOutput { output, .. } = effects.into_iter().next().unwrap() else {
            panic!("panel output");
        };
        output.unwrap()
    }
    fn hide(window: &mut crate::ui_presenters::WindowPresenter) -> Vec<UiEffect> {
        let Handling::Handled(effects) = window.handle(UiEvent::Window {
            view: PresenterId::Settings,
            event: crate::presentation::PresentationEvent::Hide,
        }) else {
            panic!("hide must be handled");
        };
        effects
    }
    for kind in [PanelKind::Persona, PanelKind::Vrm] {
        let mut window = crate::ui_presenters::WindowPresenter::new(PresenterId::Settings);
        let mut output = input(
            &mut window,
            kind,
            PanelEvent::Mount {
                value: if kind == PanelKind::Persona {
                    json!({"builtin":false,"originalId":"custom","draft":{"id":"custom","displayName":"Name","body":"Body"}})
                } else {
                    Value::Null
                },
            },
        );
        if kind == PanelKind::Persona {
            output = input(
                &mut window,
                kind,
                PanelEvent::Action {
                    name: "save".into(),
                    value: Value::Null,
                },
            );
        }
        while output.state["busy"] == true {
            assert!(hide(&mut window).is_empty());
            output = input(
                &mut window,
                kind,
                PanelEvent::Completed {
                    id: output.commands[0].id,
                    result: IoResult {
                        ok: true,
                        value: if kind == PanelKind::Vrm {
                            json!({"name":null,"quality":"medium"})
                        } else {
                            Value::Null
                        },
                        error: None,
                        issues: None,
                    },
                },
            );
        }
        assert!(hide(&mut window).iter().any(|effect| matches!(
            effect,
            UiEffect::View {
                command: crate::ui_events::ViewCommand::Hide,
                ..
            }
        )));
    }
}

#[test]
fn child_busy_changes_reproject_parent_escape_without_another_parent_input() {
    let mut s = settings();
    s.action("openVrm", Value::Null);
    let initial = child_event(
        &mut s,
        PanelKind::Vrm,
        PanelEvent::Mount { value: Value::Null },
    );
    assert_eq!(initial.updates[0].session, "test");
    assert_eq!(initial.updates[0].state["escapeEnabled"], false);
    let revision = initial.updates[0].revision;
    let loaded = child_done(
        &mut s,
        PanelKind::Vrm,
        &initial.commands[0],
        json!({"name":null,"quality":"medium"}),
    );
    assert_eq!(loaded.updates[0].state["escapeEnabled"], true);
    assert!(loaded.updates[0].revision > revision);
    let saving = child_action(&mut s, PanelKind::Vrm, "remove", Value::Null);
    assert_eq!(saving.updates[0].state["escapeEnabled"], false);
    let failed = child_event(
        &mut s,
        PanelKind::Vrm,
        PanelEvent::Completed {
            id: saving.commands[0].id,
            result: IoResult {
                ok: false,
                value: Value::Null,
                error: Some(IoError {
                    message: "failed".into(),
                    issues: None,
                }),
                issues: None,
            },
        },
    );
    assert_eq!(failed.updates[0].state["escapeEnabled"], true);
    s.action("closeVrm", Value::Null);
    assert_eq!(s.last.state["vrmControlsOpen"], false);
}

#[test]
fn delayed_external_reflection_rebases_only_the_edited_field_for_every_input_route() {
    for route in ["change", "save", "close", "escape"] {
        let mut s = settings();
        let mut external = config();
        external["revision"] = json!(2);
        external["ui"]["theme"] = json!("dark");
        s.action("snapshot", observation(external));
        let delayed = s.command("reflect");
        let mut local = config();
        local["companion"]["displayName"] = json!("edited");
        let input = draft(local, 1);
        match route {
            "change" => {
                s.event(PanelEvent::Change { value: input });
            }
            "escape" => {
                s.action(
                    "escape",
                    json!({"key":"Escape","composing":false,"keyCode":27,"draft":input}),
                );
            }
            _ => {
                s.action(route, input);
            }
        }
        let corrected = s.command("reflect");
        assert_eq!(corrected.payload["config"]["ui"]["theme"], "dark");
        assert_eq!(
            corrected.payload["config"]["companion"]["displayName"],
            "edited"
        );
        assert!(
            corrected.payload["generation"].as_u64().unwrap()
                > delayed.payload["generation"].as_u64().unwrap()
        );
        if route != "save" {
            s.action("save", Value::Null);
        }
        let save = s.command("save");
        assert_eq!(save.payload["baseConfigRevision"], 2);
        assert_eq!(
            save.payload["patch"],
            json!({"companion":{"displayName":"edited"}})
        );
    }
}

#[test]
fn settings_raw_edit_deltas_support_undo_and_new_reflection_basis() {
    let mut s = settings();
    let mut external = config();
    external["revision"] = json!(2);
    external["ui"]["theme"] = json!("dark");
    s.action("snapshot", observation(external));
    let mut edited = config();
    edited["companion"]["displayName"] = json!("first");
    s.event(PanelEvent::Change {
        value: draft(edited, 1),
    });
    s.event(PanelEvent::Change {
        value: draft(config(), 2),
    });
    assert_eq!(s.last.state["dirty"], false);
    let reflect = s.command("reflect");
    assert_eq!(reflect.payload["config"]["ui"]["theme"], "dark");
    let mut next = reflect.payload["config"].clone();
    next["companion"]["displayName"] = json!("next");
    let mut input = draft(next, 3);
    input["basis"] = json!({"fields":reflect.payload["fields"],"configRevision":reflect.payload["configRevision"],"generation":reflect.payload["generation"],"avatarImage":null,"avatarFileName":null});
    s.event(PanelEvent::Change {
        value: input.clone(),
    });
    assert!(s.last.commands.is_empty());
    s.action("save", input);
    assert_eq!(
        s.command("save").payload["patch"],
        json!({"companion":{"displayName":"next"}})
    );
}

#[test]
fn delayed_form_input_does_not_resubmit_a_saved_avatar() {
    let mut s = settings();
    let mut input = draft(config(), 1);
    input["avatarImage"] = json!([1, 2, 3]);
    input["avatarFileName"] = json!("avatar.png");
    s.action("save", input.clone());
    let save = s.command("save");
    let mut saved = config();
    saved["revision"] = json!(2);
    saved["ui"]["avatarPath"] = json!("state/avatar.png");
    s.done(&save, saved);
    input["revision"] = json!(2);
    input["fields"]["displayName"] =
        json!({"value":"after","patch":{"companion":{"displayName":"after"}}});
    s.action("save", input);
    let save = s.command("save");
    assert!(save.payload["avatarImage"].is_null());
    assert_eq!(
        save.payload["patch"],
        json!({"companion":{"displayName":"after"}})
    );
}

fn text_draft(text: &str, revision: u64) -> Value {
    let mut candidate = config();
    candidate["companion"]["displayName"] = json!(if text.trim().is_empty() {
        "Coo"
    } else {
        text.trim()
    });
    let mut input = draft(candidate, revision);
    input["fields"]["displayName"]["value"] = json!(text);
    input
}

#[test]
fn settings_pending_external_reflection_preserves_name_text_for_every_input_route() {
    for text in ["Jane ", ""] {
        for route in ["change", "save", "close", "escape"] {
            let mut s = settings();
            let mut external = config();
            external["revision"] = json!(2);
            external["ui"]["theme"] = json!("dark");
            s.action("snapshot", observation(external));
            let input = text_draft(text, 1);
            match route {
                "change" => {
                    s.event(PanelEvent::Change { value: input });
                }
                "escape" => {
                    s.action(
                        "escape",
                        json!({"key":"Escape","composing":false,"keyCode":27,"draft":input}),
                    );
                }
                _ => {
                    s.action(route, input);
                }
            }
            let reflected = s.command("reflect");
            assert_eq!(reflected.payload["fields"]["displayName"]["value"], text);
            assert_eq!(reflected.payload["config"]["ui"]["theme"], "dark");
            assert_eq!(s.last.state["dirty"], true);
            if route != "save" {
                s.action("save", Value::Null);
            }
            let save = s.command("save");
            assert_eq!(
                save.payload["patch"],
                if text.is_empty() {
                    json!({})
                } else {
                    json!({"companion":{"displayName":"Jane"}})
                }
            );
            assert_eq!(save.payload["baseConfigRevision"], 2);
        }
    }
}

#[test]
fn settings_save_normalizes_only_the_submitted_name_text() {
    let mut s = settings();
    s.action("save", text_draft("Jane ", 1));
    let save = s.command("save");
    s.event(PanelEvent::Change {
        value: text_draft("Jane  ", 2),
    });
    let mut saved = config();
    saved["revision"] = json!(2);
    saved["companion"]["displayName"] = json!("Jane");
    s.done(&save, saved.clone());
    assert_eq!(
        s.command("reflect").payload["fields"]["displayName"]["value"],
        "Jane  "
    );
    assert_eq!(s.last.state["dirty"], true);
    s.action("save", Value::Null);
    let save = s.command("save");
    s.done(&save, saved);
    assert_eq!(
        s.command("reflect").payload["fields"]["displayName"]["value"],
        "Jane"
    );
    assert_eq!(s.last.state["dirty"], false);
    let mut late = text_draft("Jane  ", 3);
    late["fields"]["uiTheme"] = json!({"value":"dark","patch":{"ui":{"theme":"dark"}}});
    s.event(PanelEvent::Change { value: late });
    assert_eq!(
        s.command("reflect").payload["fields"]["displayName"]["value"],
        "Jane"
    );
    s.action("save", Value::Null);
    assert_eq!(
        s.command("save").payload["patch"],
        json!({"ui":{"theme":"dark"}})
    );
}

#[test]
fn settings_conflict_preserves_raw_name_but_adopts_an_unedited_external_name() {
    for raw in [Some("Jane "), Some(""), None] {
        let mut s = settings();
        let mut input = text_draft(raw.unwrap_or("Coo"), 1);
        input["fields"]["downscaleWidth"] =
            json!({"value":"640","patch":{"watch":{"downscaleWidth":640}}});
        s.action("save", input);
        let save = s.command("save");
        s.fail(
            &save,
            coosenpai_core::locale::text(
                coosenpai_core::locale::TextKey::ConfigRevisionConflict,
                coosenpai_core::locale::Locale::Ja,
            ),
        );
        let reload = s.command("reloadConflict");
        let mut latest = config();
        latest["revision"] = json!(2);
        latest["ui"]["theme"] = json!("dark");
        latest["companion"]["displayName"] = json!("external");
        s.done(&reload, latest);
        let reflected = s.command("reflect");
        assert_eq!(
            reflected.payload["fields"]["displayName"]["value"],
            raw.unwrap_or("external")
        );
        assert_eq!(
            reflected.payload["config"]["companion"]["displayName"],
            match raw {
                Some("") => "Coo",
                Some(_) => "Jane",
                None => "external",
            }
        );
        assert_eq!(reflected.payload["config"]["ui"]["theme"], "dark");
    }
}

#[test]
fn settings_persona_save_does_not_normalize_unsaved_name_text() {
    let mut s = settings();
    s.event(PanelEvent::Change {
        value: text_draft("Jane ", 1),
    });
    s.action("selectPersona", json!("other"));
    let save = s.command("selectPersona");
    let mut saved = config();
    saved["revision"] = json!(2);
    saved["companion"]["persona"] = json!("other");
    s.done(&save, saved);
    assert_eq!(
        s.command("reflect").payload["fields"]["displayName"]["value"],
        "Jane "
    );
    assert_eq!(s.last.state["dirty"], true);
}

#[test]
fn settings_field_merge_preserves_every_value_kind_without_field_name_rules() {
    for (baseline, raw, normalized) in [
        (json!("old"), json!("new "), json!("new")),
        (json!(0), json!(""), json!(0)),
        (json!("queue"), json!("append"), json!("append")),
        (json!(true), json!(false), json!(false)),
        (json!([]), json!([{"id":"new"}]), json!([{"id":"new"}])),
        (Value::Null, json!("path"), json!("path")),
    ] {
        let mut base = config();
        base["custom"] = json!({"input":baseline});
        let mut input = draft(base.clone(), 0);
        input["basis"]["fields"] = form_fields(&base);
        let mut s = Screen::new(
            PanelKind::Settings,
            json!({"observation":observation(base.clone()),"draft":input}),
        );
        let mut external = base.clone();
        external["revision"] = json!(2);
        external["ui"]["theme"] = json!("dark");
        s.action("snapshot", observation(external.clone()));
        input["revision"] = json!(1);
        input["fields"]["custom.input"] =
            json!({"value":raw,"patch":{"custom":{"input":normalized}}});
        s.event(PanelEvent::Change { value: input });
        let reflected = s.command("reflect");
        assert_eq!(reflected.payload["fields"]["custom.input"]["value"], raw);
        assert_eq!(reflected.payload["config"]["custom"]["input"], normalized);
        assert_eq!(reflected.payload["config"]["ui"]["theme"], "dark");
        for (key, field) in form_fields(&external).as_object().unwrap() {
            if key != "custom.input" {
                assert_eq!(&reflected.payload["fields"][key], field);
            }
        }
        assert_eq!(s.last.state["dirty"], true);
        s.action("save", Value::Null);
        let save = s.command("save");
        assert_eq!(
            save.payload["patch"],
            if baseline == normalized {
                json!({})
            } else {
                json!({"custom":{"input":normalized}})
            }
        );
    }
}

#[test]
fn details_reset_commands_track_completion_and_reject_hidden_and_unmounted_results() {
    let mut s = Screen::new(PanelKind::Details, Value::Null);
    s.action("snapshot", snapshot(1));
    s.action("resetEmotions", Value::Null);
    let first = s.command("resetEmotions");
    assert_eq!(s.last.state["resetting"], true);
    s.action("resetEmotions", Value::Null);
    assert!(s.last.commands.is_empty());
    s.fail(&first, "reset failed");
    assert_eq!(s.last.state["resetError"], "reset failed");
    assert_eq!(s.last.state["resetting"], false);
    s.action("resetEmotions", Value::Null);
    let retry = s.command("resetEmotions");
    assert!(retry.id > first.id);
    s.action("snapshot", snapshot(3));
    s.done(&retry, snapshot(2));
    assert_eq!(s.last.state["snapshot"]["revision"], 3);
    assert_eq!(s.last.state["resetting"], false);
    assert!(s.last.state["resetError"].is_null());
    s.action("resetEmotions", Value::Null);
    let hidden = s.command("resetEmotions");
    s.panels.details_hidden();
    s.done(&hidden, snapshot(4));
    assert_eq!(s.last.state["snapshot"]["revision"], 3);
    assert_eq!(s.last.state["resetting"], false);
    s.fail(&hidden, "late error");
    assert!(s.last.state["resetError"].is_null());
    s.event(PanelEvent::Unmount);
    s.done(&hidden, snapshot(5));
    assert!(s.last.state.is_null());
    assert!(s.last.commands.is_empty());
}
