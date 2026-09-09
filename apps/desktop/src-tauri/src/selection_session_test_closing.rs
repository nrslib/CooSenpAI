use super::*;

fn observed(at: Instant) -> SelectionSession {
    let mut model = SelectionSession::default();
    model.transition(SessionInput::ShortcutPosted, at);
    assert_eq!(
        model.transition(SessionInput::Windows(true), at),
        SessionAction::Opened
    );
    model
}

#[test]
fn close_completes_on_escape_and_draining_timeout_is_only_an_error() {
    let at = Instant::now();
    for age_ms in [0, 200, 1000] {
        let mut model = observed(at);
        let close_at = at + Duration::from_millis(age_ms);
        assert_eq!(
            model.transition(SessionInput::Close, close_at),
            SessionAction::Escape
        );
        assert_eq!(model.state, SessionState::Closing);
        assert_eq!(model.deadline, Some(close_at + TRANSITION_TIMEOUT));
        assert_eq!(
            model.transition(SessionInput::EscapePosted, close_at),
            SessionAction::Cancelled
        );
        assert_eq!(model.state, SessionState::Draining);
        for elapsed in [500, 1000, 5000, 5999] {
            let now = close_at + Duration::from_millis(elapsed);
            assert_eq!(
                model.transition(SessionInput::Close, now),
                SessionAction::CloseAccepted
            );
            assert_eq!(
                model.transition(SessionInput::Windows(true), now),
                SessionAction::Observe
            );
            assert_eq!(model.deadline, Some(close_at + Duration::from_secs(6)));
        }
        assert!(matches!(
            model.transition(SessionInput::Timeout, close_at + Duration::from_secs(6)),
            SessionAction::Failed(_)
        ));
        assert_eq!(model.state, SessionState::Closed);
        assert_eq!(model.wake_at(), None);
    }
}

#[test]
fn delayed_escape_disappearance_only_completes_draining() {
    let at = Instant::now();
    let mut model = observed(at);
    model.transition(SessionInput::Close, at);
    model.transition(SessionInput::EscapePosted, at);
    assert_eq!(
        model.transition(SessionInput::Windows(false), at + Duration::from_secs(5)),
        SessionAction::Drained
    );
    assert_eq!(model.wake_at(), None);
}

#[tokio::test(start_paused = true)]
async fn escape_timeout_fails_both_pending_replies_without_a_successful_completion() {
    let port = RecordingPort::default();
    let (session, actor) = port.start();
    let result = open(&session, &port, 1);
    port.recorded("post", 1).await;
    port.observation.lock().unwrap().windows = vec![100];
    port.recorded("opened:1", 1).await;
    let (_release, gate) = oneshot::channel();
    port.observation.lock().unwrap().escape_gate = Some(gate);
    let at = Instant::now();
    assert!(close(&session, 2)
        .await
        .unwrap()
        .unwrap_err()
        .contains("1秒"));
    assert!(result.await.unwrap().err().unwrap().contains("1秒"));
    assert_eq!(Instant::now() - at, TRANSITION_TIMEOUT);
    port.recorded("ERROR selection-session: generation=2 error=選択窓への Esc 送出が1秒以内に完了しませんでした", 1).await;
    assert!(!port
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call.contains("段階=complete") || call.contains("段階=selection-closed")));
    drop(session);
    actor.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn runtime_acknowledges_escape_without_waiting_and_only_logs_six_second_timeout() {
    for disappears in [false, true] {
        let port = RecordingPort::default();
        let (session, actor) = port.start();
        let result = open(&session, &port, 1);
        port.recorded("post", 1).await;
        port.observation.lock().unwrap().windows = vec![100];
        port.recorded("opened:1", 1).await;
        let started = Instant::now();
        close(&session, 2).await.unwrap().unwrap();
        assert_eq!(Instant::now(), started);
        assert!(result.await.unwrap().unwrap().is_none());
        tokio::time::advance(Duration::from_secs(5)).await;
        if disappears {
            port.observation.lock().unwrap().windows.clear();
        }
        if disappears {
            port.recorded("範囲選択: 段階=selection-closed image=false", 1)
                .await;
        } else {
            tokio::time::advance(Duration::from_secs(1)).await;
            port.recorded("ERROR selection-session: generation=2 error=選択窓の後始末を6秒以内に確認できませんでした", 1).await;
        }
        assert_eq!(*port.escape_times.lock().unwrap(), [started]);
        assert_eq!(
            port.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.starts_with("ERROR selection-session:"))
                .count(),
            usize::from(!disappears)
        );
        drop(session);
        actor.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn shutdown_does_not_wait_for_selection_window_disappearance() {
    let port = RecordingPort::default();
    let (session, actor) = port.start();
    let mut events = session.open(1, CaptureKind::Image).unwrap();
    port.recorded("post", 1).await;
    port.observation.lock().unwrap().windows = vec![100];
    assert!(matches!(events.next().await.unwrap(), SessionEvent::Opened));
    let started = Instant::now();
    drop(session);
    actor.await.unwrap();
    assert_eq!(Instant::now(), started);
    assert_eq!(*port.escape_times.lock().unwrap(), [started]);
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_opening_posts_escape_without_waiting_for_observation() {
    let port = RecordingPort::default();
    let (session, actor) = port.start();
    let _events = session.open(1, CaptureKind::Image).unwrap();
    port.recorded("post", 1).await;
    let at = Instant::now();
    session.close(2, true).await.unwrap();
    assert_eq!(Instant::now(), at);
    assert_eq!(*port.escape_times.lock().unwrap(), [at]);
    assert!(!port
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|call| call == "observe"));
    drop(session);
    actor.await.unwrap();
}
