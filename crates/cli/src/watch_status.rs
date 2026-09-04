use super::{
    next_send_seconds, ocr_disabled_reason, trigger_label, CaptureDisposition, WatchState,
};
use chrono::{DateTime, Local, Utc};
use coosenpai_core::config::Config;
use coosenpai_core::runtime::{RuntimeErrorKind, RuntimePhase, RuntimeSnapshot};
use coosenpai_core::state::ActivityTriggerKind;
use coosenpai_core::watch_coordinator::{
    companion_retry_status, delivery_backpressure_status, format_watch_status_lines,
    WatchStatusLines,
};

pub(super) struct WatchStatus {
    provider: String,
    model: String,
    effort: String,
    pub(super) phase: &'static str,
    pub(super) trigger: &'static str,
    pub(super) ai_calls_today: u32,
    last_capture: Option<DateTime<Utc>>,
    pub(super) ocr_enabled: bool,
    pub(super) ocr_disabled_reason: Option<&'static str>,
    companion_name: String,
    companion_phase: &'static str,
    companion_retry_status: Option<String>,
    delivery_backpressure_status: Option<String>,
    pub(super) runtime_error: Option<String>,
    fullscreen: bool,
    last_report_key: Option<WatchStatusKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchStatusKey {
    provider: String,
    model: String,
    effort: String,
    phase: &'static str,
    trigger: &'static str,
    ai_calls_today: u32,
    last_capture: Option<DateTime<Utc>>,
    ocr_enabled: bool,
    ocr_disabled_reason: Option<&'static str>,
    frame_count: usize,
    companion_name: String,
    companion_phase: &'static str,
    companion_retry_status: Option<String>,
    delivery_backpressure_status: Option<String>,
    runtime_error: Option<String>,
    targets: Vec<super::TargetStatus>,
    fullscreen: bool,
}

impl WatchStatus {
    pub(super) fn new(config: &Config, helper_available: bool) -> Self {
        Self {
            provider: config.observer.provider.clone(),
            model: config.observer.model.clone(),
            effort: config.observer.effort.clone(),
            phase: "待機中",
            trigger: "なし",
            ai_calls_today: 0,
            last_capture: None,
            ocr_enabled: config.watch.ocr_gate.enabled && helper_available,
            ocr_disabled_reason: ocr_disabled_reason(config, helper_available),
            companion_name: config.companion.display_name.clone(),
            companion_phase: "待機中",
            companion_retry_status: None,
            delivery_backpressure_status: None,
            runtime_error: None,
            fullscreen: config.watch.fullscreen,
            last_report_key: None,
        }
    }

    pub(super) fn update_config(&mut self, config: &Config, helper_available: bool) {
        self.provider = config.observer.provider.clone();
        self.model = config.observer.model.clone();
        self.effort = config.observer.effort.clone();
        self.ocr_enabled = config.watch.ocr_gate.enabled && helper_available;
        self.ocr_disabled_reason = ocr_disabled_reason(config, helper_available);
        self.fullscreen = config.watch.fullscreen;
    }

    pub(super) fn update_runtime_snapshot(&mut self, snapshot: &RuntimeSnapshot) {
        self.companion_name
            .clone_from(&snapshot.companion_display_name);
        self.companion_phase = if snapshot.last_error.is_some() {
            "エラー"
        } else if snapshot.phase == RuntimePhase::Companion {
            "考え中"
        } else {
            "待機中"
        };
        self.companion_retry_status = companion_retry_status(snapshot);
        self.delivery_backpressure_status = delivery_backpressure_status(snapshot);
        self.runtime_error = snapshot.last_error.as_ref().and_then(|error| {
            (error.kind == RuntimeErrorKind::Config).then(|| {
                format!(
                    "設定エラーで停止中: {}",
                    error.message.as_deref().unwrap_or("設定を修正してください")
                )
            })
        });
    }

    pub(super) fn should_report(&mut self, state: &WatchState) -> bool {
        let key = WatchStatusKey {
            provider: self.provider.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
            phase: self.phase,
            trigger: self.trigger,
            ai_calls_today: self.ai_calls_today,
            last_capture: self.last_capture,
            ocr_enabled: self.ocr_enabled,
            ocr_disabled_reason: self.ocr_disabled_reason,
            frame_count: state.pending_frames.len(),
            companion_name: self.companion_name.clone(),
            companion_phase: self.companion_phase,
            companion_retry_status: self.companion_retry_status.clone(),
            delivery_backpressure_status: self.delivery_backpressure_status.clone(),
            runtime_error: self.runtime_error.clone(),
            targets: state.target_statuses.clone(),
            fullscreen: self.fullscreen,
        };
        if self.last_report_key.as_ref() == Some(&key) {
            return false;
        }
        self.last_report_key = Some(key);
        true
    }

    pub(super) fn report(&mut self, config: &Config, state: &WatchState) {
        if !self.should_report(state) {
            return;
        }
        let last_capture = self
            .last_capture
            .map(|value| value.with_timezone(&Local).format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "未撮影".to_owned());
        for line in format_watch_status_lines(&WatchStatusLines {
            phase: self.phase,
            provider: &self.provider,
            model: &self.model,
            effort: &self.effort,
            ai_calls_today: self.ai_calls_today,
            last_capture: &last_capture,
            trigger: self.trigger,
            next_send_seconds: next_send_seconds(config, state),
            frame_count: state.pending_frames.len(),
            ocr_enabled: self.ocr_enabled,
            ocr_disabled_reason: self.ocr_disabled_reason,
            companion_retry: self.companion_retry_status.as_deref(),
            delivery_backpressure: self.delivery_backpressure_status.as_deref(),
            runtime_error: self.runtime_error.as_deref(),
            companion_name: &self.companion_name,
            companion_phase: self.companion_phase,
        }) {
            println!("{line}");
        }
        println!(
            "対象: フルスクリーン | {} | 最後の撮影: {} | きっかけ: {}",
            if config.watch.fullscreen {
                "有効"
            } else {
                "無効"
            },
            last_capture,
            self.trigger
        );
        for target in &state.target_statuses {
            let captured = target
                .last_captured_at
                .map(|value| value.with_timezone(&Local).format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "未撮影".to_owned());
            let foreground = if target.foreground {
                "前面"
            } else {
                "背面"
            };
            let enabled = if target.enabled { "有効" } else { "無効" };
            println!(
                "対象: {} | {} | {} | 最後の撮影: {} | きっかけ: {}",
                target.name,
                enabled,
                foreground,
                captured,
                target.trigger.unwrap_or("なし")
            );
        }
    }

    pub(super) fn report_capture(
        &mut self,
        config: &Config,
        state: &WatchState,
        disposition: CaptureDisposition,
        trigger: ActivityTriggerKind,
    ) {
        self.trigger = trigger_label(trigger);
        match disposition {
            CaptureDisposition::Accepted => {
                self.phase = "撮影中";
                self.last_capture = state.last_captured_at;
            }
            CaptureDisposition::Unchanged => self.phase = "待機中",
            CaptureDisposition::Suppressed => self.phase = "撮影抑止",
        }
        self.report(config, state);
    }

    pub(super) fn report_observation_start(&mut self, config: &Config, state: &WatchState) {
        self.phase = "AI処理中";
        self.report(config, state);
    }

    pub(super) fn report_idle(&mut self, config: &Config, state: &WatchState) {
        self.phase = "待機中";
        self.report(config, state);
    }
}
