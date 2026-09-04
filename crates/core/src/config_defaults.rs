use super::PENDING_DELIVERY_ITEM_MAX_BYTES;

pub(super) fn default_true() -> bool {
    true
}
pub(super) fn default_send_interval() -> u64 {
    60_000
}
pub(super) fn default_send_debounce() -> u64 {
    2_000
}
pub(super) fn default_frames_per_send() -> usize {
    4
}
pub(super) fn default_downscale_width() -> u32 {
    1_280
}
pub(super) fn default_typing_pause() -> u64 {
    2_000
}
pub(super) fn default_active_threshold() -> u64 {
    1_000
}
pub(super) fn default_app_switch_settle() -> u64 {
    1_500
}
pub(super) fn default_max_interval() -> u64 {
    60_000
}
pub(super) fn default_min_spacing() -> u64 {
    5_000
}
pub(super) fn default_poll() -> u64 {
    1_000
}
pub(super) fn default_battery_multiplier() -> f64 {
    2.0
}
pub(super) fn default_ocr_level() -> String {
    "accurate".to_owned()
}
pub(super) fn default_ocr_timeout() -> u64 {
    3_000
}
pub(super) fn default_codex() -> String {
    "codex".to_owned()
}
pub(super) fn default_model() -> String {
    "default".to_owned()
}
pub(super) fn default_effort() -> String {
    "default".to_owned()
}
pub(super) fn default_agent_timeout() -> u64 {
    120_000
}
pub(super) fn default_observer_daily_limit() -> u32 {
    1_000
}
pub(super) fn default_excerpt_max_chars() -> usize {
    600
}
pub(super) fn default_excerpt_count() -> usize {
    6
}
pub(super) fn default_total_excerpt_chars() -> usize {
    2_000
}
pub(super) fn default_changes_max() -> usize {
    8
}
pub(super) fn default_persona() -> String {
    "coo-chan".to_owned()
}
pub(super) fn default_display_name() -> String {
    "Coo".to_owned()
}
pub(super) fn default_capture_shortcut() -> Option<String> {
    Some("Alt+Shift+4".to_owned())
}
pub(super) fn default_microphone_shortcut() -> Option<String> {
    Some("Alt+Space".to_owned())
}
pub(super) fn default_speech_locale() -> String {
    "system".to_owned()
}
pub(super) fn default_speech_mode() -> String {
    "toggle".to_owned()
}
pub(super) fn default_speech_input_device() -> String {
    "default".to_owned()
}
pub(super) fn default_speech_confirm_before_send() -> bool {
    true
}
pub(super) fn default_toggle_panel_shortcut() -> Option<String> {
    Some("Alt+Shift+V".to_owned())
}
pub(super) fn default_toggle_watch_shortcut() -> Option<String> {
    Some("Alt+Shift+W".to_owned())
}
pub(super) fn default_send_text_shortcut() -> Option<String> {
    Some("Alt+Shift+C".to_owned())
}
pub(super) fn default_copy_last_reply_shortcut() -> Option<String> {
    Some("Alt+Shift+Y".to_owned())
}
pub(super) fn default_text_quick_actions() -> Vec<super::PopupQuickAction> {
    [
        ("翻訳して", "翻訳して"),
        ("要約して", "要約して"),
        ("これ何？", "これ何？"),
        ("返信メールを考えて", "返信メールを考えて"),
        ("返信を作って", "この文への返信を作って"),
    ]
    .into_iter()
    .map(|(label, message)| super::PopupQuickAction {
        label: label.to_owned(),
        message: message.to_owned(),
    })
    .collect()
}
pub(super) fn default_bubble_max_stack() -> usize {
    3
}
pub(super) fn default_bubble_keep_latest() -> bool {
    false
}
pub(super) fn default_bubble_position() -> String {
    "bottom-right".to_owned()
}
pub(super) fn default_bubble_display() -> String {
    "main".to_owned()
}
pub(super) fn default_image_quick_actions() -> Vec<super::PopupQuickAction> {
    ["これ何？", "説明して", "文字を書き起こして"]
        .into_iter()
        .map(|value| super::PopupQuickAction {
            label: value.to_owned(),
            message: value.to_owned(),
        })
        .collect()
}
pub(super) fn default_send_key() -> String {
    "enter".to_owned()
}
pub(super) fn default_ui_theme() -> String {
    "system".to_owned()
}
pub(super) fn default_ui_font() -> String {
    "system".to_owned()
}
pub(super) fn default_while_thinking() -> String {
    "queue".to_owned()
}
pub(super) fn default_assertiveness() -> String {
    "normal".to_owned()
}
pub(super) fn default_session_max() -> usize {
    60
}
pub(super) fn default_wake_coalesce() -> usize {
    5
}
pub(super) fn default_stuck_after() -> u64 {
    900_000
}
pub(super) fn default_pending_delivery_limit() -> usize {
    20
}
pub(super) fn default_pending_delivery_max_bytes() -> usize {
    default_pending_delivery_limit() * PENDING_DELIVERY_ITEM_MAX_BYTES
}
pub(super) fn default_context_refresh_calls() -> usize {
    20
}
pub(super) fn default_review_time() -> String {
    "18:00".to_owned()
}
pub(super) fn default_proactive_quiet_minutes() -> u64 {
    1
}
pub(super) fn default_notification_mode() -> String {
    "bubble".to_owned()
}
pub(super) fn default_notification_min_priority() -> String {
    "info".to_owned()
}
pub(super) fn default_notification_ttl() -> u64 {
    30_000
}
pub(super) fn default_observation_retention() -> u64 {
    7
}
pub(super) fn default_conversation_retention() -> u64 {
    30
}
pub(super) fn default_memory_grace_minutes() -> u64 {
    60
}
pub(super) fn default_daily_memory_retention() -> u64 {
    90
}
pub(super) fn default_weekly_memory_retention() -> u64 {
    52
}
pub(super) fn default_memory_job_retention() -> u64 {
    30
}
pub(super) fn default_memory_source_max_bytes() -> usize {
    200 * 1_024
}
pub(super) fn default_memory_prompt_max_bytes() -> usize {
    16 * 1_024
}
pub(super) fn default_memory_fact_limit() -> usize {
    1_000
}
pub(super) fn default_memory_fact_max_bytes() -> usize {
    2 * 1_024 * 1_024
}
pub(super) fn default_memory_candidate_limit() -> usize {
    50
}
pub(super) fn default_memory_candidate_max_bytes() -> usize {
    64 * 1_024
}
pub(super) fn default_memory_storage_max_bytes() -> usize {
    10 * 1_024 * 1_024
}
pub(super) fn default_fact_prompt_daily_limit() -> u32 {
    3
}

