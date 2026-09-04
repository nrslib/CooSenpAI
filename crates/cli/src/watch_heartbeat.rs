use super::*;

pub(super) async fn heartbeat_if_due(
    config: &Config,
    runtime: &coosenpai_core::runtime::RuntimeHandle,
    state: &mut WatchState,
    max_interval_ms: u64,
) -> Result<()> {
    if !state.pending_frames.is_empty()
        || (Instant::now()
            .duration_since(state.last_observation)
            .as_millis() as u64)
            < max_interval_ms
    {
        return Ok(());
    }
    let now = Instant::now();
    let now_utc = Utc::now();
    match state.stagnation_store.load(now_utc) {
        Ok(snapshot)
            if snapshot.last_meaningful_change_at != state.last_meaningful_change_at
                || snapshot.reported != state.stagnation.is_reported() =>
        {
            state.stagnation.sync_durable_episode(
                now,
                snapshot.elapsed(now_utc),
                snapshot.reported,
            );
            state.pending_stagnation_report = snapshot.pending_report;
            state.last_meaningful_change_at = snapshot.last_meaningful_change_at;
        }
        Ok(_) => {}
        Err(error) => eprintln!("停滞状態を再同期できませんでした: {error}"),
    }
    let stagnation_candidate = state
        .stagnation
        .prepare_stagnation(now, config.companion.stuck_after_ms);
    if state.pending_stagnation_report.is_none() {
        if let Some(candidate) = stagnation_candidate {
            state.pending_stagnation_report = state.stagnation_store.prepare_report(
                state.last_meaningful_change_at,
                candidate.elapsed_ms(),
                now_utc,
            )?;
        }
    }
    let stagnation = state
        .pending_stagnation_report
        .as_ref()
        .map(|intent| StagnationObservation {
            event_id: Some(intent.id.clone()),
            event_created_at: Some(intent.created_at.clone()),
            last_meaningful_change_at: state
                .last_meaningful_change_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            elapsed_ms: intent.elapsed_ms,
            activity_signals: true,
            detail: "画面は変わっていませんが、入力やマウス操作は続いています。悩んでいる可能性があります。"
                .to_owned(),
        });
    runtime
        .heartbeat_with_stagnation_cancellable(stagnation, CancellationToken::new())
        .await?;
    if let Some(intent) = state.pending_stagnation_report.clone() {
        if state.stagnation_store.mark_reported(&intent, Utc::now())? {
            state.stagnation.mark_reported();
        }
        state.pending_stagnation_report = None;
    }
    state.last_observation = Instant::now();
    state.window_start = state.last_observation;
    Ok(())
}

pub(super) fn mark_meaningful_change(
    state: &mut WatchState,
    target: &str,
    image_hash: String,
    ocr_signature: Option<String>,
    captured_at: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let changed = state.stagnation_store.record_meaningful_change(
        target,
        StagnationFingerprint {
            image_hash,
            ocr_signature,
        },
        captured_at,
    )?;
    if !changed {
        return Ok(());
    }
    state.stagnation.mark_meaningful_change(Instant::now());
    state.pending_stagnation_report = None;
    state.last_meaningful_change_at = captured_at;
    Ok(())
}
