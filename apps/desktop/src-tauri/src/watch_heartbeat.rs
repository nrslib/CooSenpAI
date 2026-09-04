use super::*;

pub(super) async fn heartbeat_if_due(
    state: &DesktopState,
    config: &Config,
    memory: &mut WatchMemory,
    max_interval_ms: u64,
    cancellation: CancellationToken,
) -> Result<()> {
    if !memory.frames.is_empty()
        || (memory.last_observation.elapsed().as_millis() as u64) < max_interval_ms
    {
        return Ok(());
    }
    let now = Instant::now();
    let now_utc = chrono::Utc::now();
    match memory.stagnation_store.load(now_utc) {
        Ok(snapshot)
            if snapshot.last_meaningful_change_at != memory.last_meaningful_change_at
                || snapshot.reported != memory.stagnation.is_reported() =>
        {
            memory.stagnation.sync_durable_episode(
                now,
                snapshot.elapsed(now_utc),
                snapshot.reported,
            );
            memory.pending_stagnation_report = snapshot.pending_report;
            memory.last_meaningful_change_at = snapshot.last_meaningful_change_at;
        }
        Ok(_) => {}
        Err(error) => {
            let _ = state.logger.write(
                "WARN",
                &format!("停滞状態を再同期できませんでした: error-type=persistence ({error})"),
            );
        }
    }
    let stagnation_candidate = memory
        .stagnation
        .prepare_stagnation(now, config.companion.stuck_after_ms);
    if memory.pending_stagnation_report.is_none() {
        if let Some(candidate) = stagnation_candidate {
            memory.pending_stagnation_report = memory.stagnation_store.prepare_report(
                memory.last_meaningful_change_at,
                candidate.elapsed_ms(),
                now_utc,
            )?;
        }
    }
    let stagnation = memory
        .pending_stagnation_report
        .as_ref()
        .map(|intent| StagnationObservation {
            event_id: Some(intent.id.clone()),
            event_created_at: Some(intent.created_at.clone()),
            last_meaningful_change_at: memory
                .last_meaningful_change_at
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            elapsed_ms: intent.elapsed_ms,
            activity_signals: true,
            detail: "画面は変わっていませんが、入力やマウス操作は続いています。悩んでいる可能性があります。"
                .to_owned(),
        });
    let observation = state
        .core_runtime()
        .heartbeat(stagnation, cancellation.clone())
        .await
        .map_err(anyhow::Error::new)
        .context("Manager の heartbeat ACK")?;
    if let Some(intent) = memory.pending_stagnation_report.clone() {
        match memory
            .stagnation_store
            .mark_reported(&intent, chrono::Utc::now())
        {
            Ok(true) => {
                memory.stagnation.mark_reported();
                memory.pending_stagnation_report = None;
            }
            Ok(false) => memory.pending_stagnation_report = None,
            Err(error) => {
                let _ = state.logger.write(
                    "WARN",
                    &format!("停滞状態を保存できませんでした: error-type=persistence ({error})"),
                );
                return Err(error.into());
            }
        }
    }
    let _ = state
        .core_runtime()
        .process_mailbox(cancellation)
        .await
        .map_err(anyhow::Error::new)
        .context("Manager の heartbeat 後 mailbox ACK")?;
    memory.last_observation = Instant::now();
    memory.window_start = memory.last_observation;
    state
        .publish(|snapshot| snapshot.observer.record_observation(observation))
        .await;
    Ok(())
}

pub(super) fn mark_meaningful_change(
    memory: &mut WatchMemory,
    target: &str,
    image_hash: String,
    ocr_signature: Option<String>,
    captured_at: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    let changed = memory.stagnation_store.record_meaningful_change(
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
    memory.stagnation.mark_meaningful_change(Instant::now());
    memory.pending_stagnation_report = None;
    memory.last_meaningful_change_at = captured_at;
    Ok(())
}
