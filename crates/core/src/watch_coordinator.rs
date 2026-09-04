use crate::config::Config;
use crate::image_processing::ExcludedBounds;
use crate::observer::ObservationFrameInput;
use crate::ports::{ActivitySnapshot, OcrTextBlock};
use crate::runtime::RuntimeSnapshot;
use crate::state::ActivityTriggerKind;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

const RETRY_INITIAL: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

#[path = "watch_stagnation.rs"]
mod watch_stagnation;
pub use watch_stagnation::{
    StagnationCandidate, StagnationFingerprint, StagnationReportIntent, StagnationSnapshot,
    StagnationTracker, WatchStagnationStore,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrText {
    pub signature: String,
    pub text: String,
}

pub fn normalize_ocr_blocks(
    blocks: &[OcrTextBlock],
    image_width: u32,
    image_height: u32,
    excluded: &[ExcludedBounds],
    ignored_top_pixels: u32,
) -> OcrText {
    let mut filtered = blocks
        .iter()
        .filter(|block| {
            block_is_visible(
                block,
                image_width,
                image_height,
                excluded,
                ignored_top_pixels,
            )
        })
        .map(|block| (block.y, block.x, normalize_ocr_line(&block.text)))
        .filter(|(_, _, text)| !text.is_empty())
        .collect::<Vec<_>>();
    filtered.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
    });
    let full_text = filtered
        .into_iter()
        .map(|(_, _, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    let mut hasher = Sha256::new();
    hasher.update(full_text.as_bytes());
    OcrText {
        signature: format!("{:x}", hasher.finalize()),
        text: full_text.chars().take(2_000).collect(),
    }
}

fn block_is_visible(
    block: &OcrTextBlock,
    image_width: u32,
    image_height: u32,
    excluded: &[ExcludedBounds],
    ignored_top_pixels: u32,
) -> bool {
    let left = block.x * f64::from(image_width);
    let top = block.y * f64::from(image_height);
    let right = left + block.width * f64::from(image_width);
    let bottom = top + block.height * f64::from(image_height);
    top >= f64::from(ignored_top_pixels)
        && !excluded.iter().any(|bounds| {
            left < bounds.x + bounds.width
                && right > bounds.x
                && top < bounds.y + bounds.height
                && bottom > bounds.y
        })
}

fn normalize_ocr_line(value: &str) -> String {
    remove_clock_tokens(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn remove_clock_tokens(value: &str) -> String {
    let characters = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < characters.len() {
        if let Some(length) = clock_token_length(&characters, index) {
            index += length;
        } else {
            output.push(characters[index]);
            index += 1;
        }
    }
    output
}

fn clock_token_length(characters: &[char], start: usize) -> Option<usize> {
    let mut index = start;
    let first_digits = take_ascii_digits(characters, index, 2);
    if !(1..=2).contains(&first_digits) {
        return None;
    }
    index += first_digits;
    if characters.get(index) != Some(&':') {
        return None;
    }
    index += 1;
    if take_ascii_digits(characters, index, 2) != 2 {
        return None;
    }
    index += 2;
    if characters.get(index) == Some(&':') {
        index += 1;
        if take_ascii_digits(characters, index, 2) != 2 {
            return None;
        }
        index += 2;
    }
    Some(index - start)
}

fn take_ascii_digits(characters: &[char], start: usize, maximum: usize) -> usize {
    characters
        .iter()
        .skip(start)
        .take(maximum)
        .take_while(|character| character.is_ascii_digit())
        .count()
}

pub fn effective_max_interval_ms(config: &Config, on_battery: bool) -> u64 {
    crate::timing::effective_interval_ms(
        config.watch.triggers.max_interval_ms,
        config.watch.battery.enabled,
        on_battery,
        config.watch.battery.multiplier,
    )
}

pub fn watch_send_due(
    config: &Config,
    frame_count: usize,
    since_last_frame_ms: u64,
    since_window_start_ms: u64,
) -> bool {
    crate::timing::send_due(
        frame_count,
        config.watch.frames_per_send,
        since_last_frame_ms,
        config.watch.send_debounce_ms,
        since_window_start_ms,
        config.watch.send_interval_ms,
    )
}

pub fn next_send_seconds(
    config: &Config,
    since_last_frame_ms: Option<u64>,
    since_window_start_ms: u64,
) -> u64 {
    let Some(since_last_frame_ms) = since_last_frame_ms else {
        return crate::timing::remaining_seconds(config.watch.send_interval_ms);
    };
    let debounce = config
        .watch
        .send_debounce_ms
        .saturating_sub(since_last_frame_ms);
    let interval = config
        .watch
        .send_interval_ms
        .saturating_sub(since_window_start_ms);
    crate::timing::remaining_seconds(debounce.min(interval))
}

pub fn is_self_application(name: &str) -> bool {
    name.eq_ignore_ascii_case("coosenpai")
}

#[derive(Debug, Clone)]
pub struct TriggerCoordinator {
    input_active: bool,
    last_app: Option<String>,
    last_triggered_app: Option<String>,
    pending_app_switch_since: Option<Instant>,
}

impl TriggerCoordinator {
    pub fn new(config: &Config, initial: Option<&ActivitySnapshot>) -> Self {
        let last_app = initial.and_then(|value| value.front_app.clone());
        Self {
            input_active: initial
                .is_some_and(|value| value.idle_ms < config.watch.triggers.active_threshold_ms),
            last_triggered_app: last_app.clone(),
            last_app,
            pending_app_switch_since: None,
        }
    }

    pub fn evaluate(
        &mut self,
        config: &Config,
        activity: Option<&ActivitySnapshot>,
        now: Instant,
        elapsed_since_capture_ms: u64,
        effective_max_interval_ms: u64,
    ) -> Option<ActivityTriggerKind> {
        let mut trigger = None;
        let mut app_switch_detected = false;
        if let Some(activity) = activity {
            if activity.idle_ms < config.watch.triggers.active_threshold_ms {
                self.input_active = true;
            }
            if self.input_active
                && crate::timing::typing_pause_reached(
                    activity.idle_ms,
                    config.watch.triggers.typing_pause_ms,
                )
            {
                self.input_active = false;
                trigger = Some(ActivityTriggerKind::TypingPaused);
            }
            if self.last_app != activity.front_app {
                self.last_app = activity.front_app.clone();
                if config.watch.triggers.app_switch {
                    self.pending_app_switch_since = Some(now);
                    app_switch_detected = true;
                }
            }
        } else {
            self.input_active = false;
        }
        if app_switch_detected {
            return None;
        }
        if !config.watch.triggers.app_switch {
            self.pending_app_switch_since = None;
            self.last_triggered_app = self.last_app.clone();
        } else if activity.is_some() {
            if let Some(since) = self.pending_app_switch_since {
                let elapsed_ms = now.saturating_duration_since(since).as_millis() as u64;
                if crate::timing::settle_elapsed(
                    elapsed_ms,
                    config.watch.triggers.app_switch_settle_ms,
                ) {
                    self.pending_app_switch_since = None;
                    if self.last_triggered_app != self.last_app {
                        self.last_triggered_app = self.last_app.clone();
                        trigger = Some(ActivityTriggerKind::AppSwitched);
                    }
                }
            }
        }
        if trigger.is_none() && elapsed_since_capture_ms >= effective_max_interval_ms {
            trigger = Some(ActivityTriggerKind::Timer);
        }
        match trigger {
            Some(ActivityTriggerKind::AppSwitched | ActivityTriggerKind::TypingPaused) => trigger,
            Some(ActivityTriggerKind::Timer) if !self.input_active => trigger,
            _ => None,
        }
    }

    pub fn reset_after_resume(&mut self) {
        self.input_active = false;
        self.pending_app_switch_since = None;
    }

    pub fn input_active(&self) -> bool {
        self.input_active
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityPollDecision {
    pub trigger: Option<ActivityTriggerKind>,
    pub front_app: Option<String>,
}

pub fn evaluate_activity_poll(
    coordinator: &mut TriggerCoordinator,
    config: &Config,
    activity: Option<&ActivitySnapshot>,
    now: Instant,
    elapsed_since_capture_ms: u64,
    effective_max_interval_ms: u64,
) -> ActivityPollDecision {
    let trigger = coordinator.evaluate(
        config,
        activity,
        now,
        elapsed_since_capture_ms,
        effective_max_interval_ms,
    );
    ActivityPollDecision {
        trigger,
        front_app: activity.and_then(|value| value.front_app.clone()),
    }
}

#[derive(Debug, Clone)]
pub struct ApplicationTriggerCoordinator {
    input_active: bool,
    was_foreground: bool,
}

impl ApplicationTriggerCoordinator {
    pub fn new(
        config: &Config,
        target_bundle_id: &str,
        target_name: &str,
        initial: Option<&ActivitySnapshot>,
    ) -> Self {
        let foreground = application_is_foreground(initial, target_bundle_id, target_name);
        Self {
            input_active: foreground
                && initial
                    .is_some_and(|value| value.idle_ms < config.watch.triggers.active_threshold_ms),
            was_foreground: foreground,
        }
    }

    pub fn evaluate(
        &mut self,
        config: &Config,
        target_bundle_id: &str,
        target_name: &str,
        activity: Option<&ActivitySnapshot>,
        elapsed_since_capture_ms: u64,
        effective_max_interval_ms: u64,
    ) -> Option<ActivityTriggerKind> {
        let foreground = application_is_foreground(activity, target_bundle_id, target_name);
        let became_foreground = foreground && !self.was_foreground;
        self.was_foreground = foreground;

        if foreground {
            if activity
                .is_some_and(|value| value.idle_ms < config.watch.triggers.active_threshold_ms)
            {
                self.input_active = true;
            }
            if became_foreground && config.watch.triggers.app_switch {
                return Some(ActivityTriggerKind::AppSwitched);
            }
            if self.input_active
                && activity.is_some_and(|value| {
                    crate::timing::typing_pause_reached(
                        value.idle_ms,
                        config.watch.triggers.typing_pause_ms,
                    )
                })
            {
                self.input_active = false;
                return Some(ActivityTriggerKind::TypingPaused);
            }
        } else {
            self.input_active = false;
        }

        (elapsed_since_capture_ms >= effective_max_interval_ms && !self.input_active)
            .then_some(ActivityTriggerKind::Timer)
    }

    pub fn reset_after_resume(&mut self) {
        self.input_active = false;
    }
}

pub fn application_is_foreground(
    activity: Option<&ActivitySnapshot>,
    target_bundle_id: &str,
    target_name: &str,
) -> bool {
    activity.is_some_and(|value| {
        value
            .front_app_bundle_id
            .as_deref()
            .is_some_and(|bundle_id| bundle_id == target_bundle_id)
            || value
                .front_app
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(target_name))
    })
}

pub fn application_capture_is_needed(fullscreen_enabled: bool, foreground: bool) -> bool {
    !(fullscreen_enabled && foreground)
}

pub fn frame_target_is_enabled(config: &Config, target: &str) -> bool {
    if target == "fullscreen" {
        return config.watch.fullscreen;
    }
    let Some(bundle_id) = target.strip_prefix("app:") else {
        return false;
    };
    config
        .watch
        .apps
        .iter()
        .any(|application| application.enabled && application.bundle_id == bundle_id)
}

pub fn retain_enabled_frames(config: &Config, frames: &mut Vec<ObservationFrameInput>) {
    frames.retain(|frame| frame_target_is_enabled(config, &frame.target));
}

#[derive(Debug, Clone)]
pub struct RetryBackoff {
    next_attempt: Option<Instant>,
    delay: Duration,
}

impl Default for RetryBackoff {
    fn default() -> Self {
        Self {
            next_attempt: None,
            delay: RETRY_INITIAL,
        }
    }
}

impl RetryBackoff {
    pub fn is_due(&self, now: Instant) -> bool {
        self.next_attempt.is_none_or(|next| next <= now)
    }

    pub fn defer(&mut self, now: Instant) {
        self.next_attempt = Some(now + self.delay);
        self.delay = (self.delay * 2).min(RETRY_MAX);
    }

    pub fn reset(&mut self) {
        self.next_attempt = None;
        self.delay = RETRY_INITIAL;
    }

    pub fn next_attempt(&self) -> Option<Instant> {
        self.next_attempt
    }
}

pub fn companion_retry_status(snapshot: &RuntimeSnapshot) -> Option<String> {
    let error = snapshot.last_error.as_ref()?;
    let seconds = snapshot.companion_retry_in_seconds?;
    Some(format!(
        "{}の準備に失敗（{}）: {seconds} 秒後に再試行",
        snapshot.companion_display_name,
        error.kind.as_str(),
    ))
}

pub fn delivery_backpressure_status(snapshot: &RuntimeSnapshot) -> Option<String> {
    snapshot.delivery_outbox_blocked.then(|| {
        format!(
            "配信待ち {} 件（outbox に書けません）",
            snapshot.pending_deliveries
        )
    })
}

pub struct WatchStatusLines<'a> {
    pub phase: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub effort: &'a str,
    pub ai_calls_today: u32,
    pub last_capture: &'a str,
    pub trigger: &'a str,
    pub next_send_seconds: u64,
    pub frame_count: usize,
    pub ocr_enabled: bool,
    pub ocr_disabled_reason: Option<&'a str>,
    pub companion_retry: Option<&'a str>,
    pub delivery_backpressure: Option<&'a str>,
    pub runtime_error: Option<&'a str>,
    pub companion_name: &'a str,
    pub companion_phase: &'a str,
}

pub fn format_watch_status_lines(status: &WatchStatusLines<'_>) -> Vec<String> {
    let mut lines = vec![
        format!(
            "見守り: {} | AI: {} / {} / {} | 今日のAI呼び出し: {} | 最後の撮影: {}",
            status.phase,
            status.provider,
            status.model,
            status.effort,
            status.ai_calls_today,
            status.last_capture
        ),
        format!("直近のきっかけ: {} → 撮影", status.trigger),
        format!(
            "次の送信まで {} 秒（フレーム {} 枚）",
            status.next_send_seconds, status.frame_count
        ),
        match (status.ocr_enabled, status.ocr_disabled_reason) {
            (true, _) => "OCR ゲート: 有効".to_owned(),
            (false, Some(reason)) => format!("OCR ゲート: 無効（{reason}）"),
            (false, None) => "OCR ゲート: 無効".to_owned(),
        },
        "区切り検知: 有効".to_owned(),
        format!("{}: {}", status.companion_name, status.companion_phase),
    ];
    lines.extend(
        status
            .companion_retry
            .iter()
            .map(|value| (*value).to_owned()),
    );
    lines.extend(
        status
            .delivery_backpressure
            .iter()
            .map(|value| (*value).to_owned()),
    );
    lines.extend(status.runtime_error.iter().map(|value| (*value).to_owned()));
    lines
}

