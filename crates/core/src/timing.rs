use std::time::Duration;

const TUTORIAL_RESPONSE_BASE_DELAY_MS: u64 = 1_200;
const TUTORIAL_RESPONSE_PER_CHARACTER_MS: u64 = 110;
const TUTORIAL_RESPONSE_MAX_DELAY_MS: u64 = 10_000;

pub fn typing_pause_reached(idle_ms: u64, typing_pause_ms: u64) -> bool {
    idle_ms >= typing_pause_ms
}

pub(crate) fn tutorial_response_delay(message: &str) -> Duration {
    let characters = message
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as u64;
    Duration::from_millis(
        TUTORIAL_RESPONSE_BASE_DELAY_MS
            .saturating_add(characters.saturating_mul(TUTORIAL_RESPONSE_PER_CHARACTER_MS))
            .min(TUTORIAL_RESPONSE_MAX_DELAY_MS),
    )
}

pub fn min_spacing_reached(elapsed_ms: u64, min_spacing_ms: u64) -> bool {
    elapsed_ms >= min_spacing_ms
}

pub fn send_due(
    frame_count: usize,
    frame_limit: usize,
    since_accepted_ms: u64,
    debounce_ms: u64,
    since_window_start_ms: u64,
    interval_ms: u64,
) -> bool {
    frame_count >= frame_limit
        || since_accepted_ms >= debounce_ms
        || since_window_start_ms >= interval_ms
}

pub fn settle_elapsed(elapsed_ms: u64, settle_ms: u64) -> bool {
    elapsed_ms >= settle_ms
}

pub fn effective_interval_ms(
    max_interval_ms: u64,
    battery_enabled: bool,
    on_battery: bool,
    multiplier: f64,
) -> u64 {
    if !battery_enabled || !on_battery {
        return max_interval_ms;
    }
    let scaled = max_interval_ms as f64 * multiplier;
    if !scaled.is_finite() || scaled >= u64::MAX as f64 {
        u64::MAX
    } else {
        scaled.round().max(1.0) as u64
    }
}

pub fn remaining_seconds(remaining_ms: u64) -> u64 {
    remaining_ms.saturating_add(999) / 1_000
}

