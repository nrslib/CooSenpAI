use super::*;

#[test]
fn capture_and_draining_transitions_need_events_but_no_elapsed_time_or_timeout() {
    for timeouts_enabled in [true, false] {
        let at = Instant::now();
        let mut model = SelectionSession {
            timeouts_enabled,
            ..SelectionSession::default()
        };
        for (input, expected) in [
            (SessionInput::Open, SessionAction::Start),
            (SessionInput::ShortcutPosted, SessionAction::Observe),
            (SessionInput::Windows(true), SessionAction::Opened),
            (SessionInput::ClipboardChanged(1), SessionAction::ReadImage),
            (SessionInput::ImageRead(true), SessionAction::Captured),
            (SessionInput::Windows(true), SessionAction::Observe),
            (SessionInput::ClipboardChanged(2), SessionAction::Ignore),
            (SessionInput::Windows(false), SessionAction::Drained),
            (SessionInput::Windows(false), SessionAction::Ignore),
        ] {
            assert_eq!(model.transition(input, at), expected);
        }
        assert_eq!(model.state, SessionState::Closed);
        assert_eq!(model.deadline, None);
    }
}

#[tokio::test(start_paused = true)]
async fn open_while_draining_uses_the_pre_post_ids_and_only_tracks_the_new_window() {
    let port = RecordingPort::default();
    let (session, actor) = port.start_without_timeouts();
    let first = open(&session, &port, 1);
    port.recorded("post", 1).await;
    port.observation.lock().unwrap().windows = vec![42];
    port.recorded("opened:1", 1).await;
    {
        let mut observed = port.observation.lock().unwrap();
        observed.count = 1;
        observed.image = Some(vec![1]);
    }
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(100), first)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Some(SelectionData::Image(_))
    ));
    let mut next = session.open(2, CaptureKind::Image).unwrap();
    port.recorded("post", 2).await;
    let polls = port
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.as_str() == "observe")
        .count();
    port.recorded("observe", polls + 2).await;
    assert!(matches!(
        next.0.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    port.observation.lock().unwrap().windows = vec![42, 99];
    assert!(matches!(next.next().await.unwrap(), SessionEvent::Opened));
    port.observation.lock().unwrap().windows = vec![99];
    let polls = port
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.as_str() == "observe")
        .count();
    port.recorded("observe", polls + 2).await;
    assert!(matches!(
        next.0.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    {
        let mut observed = port.observation.lock().unwrap();
        observed.count = 3;
        observed.image = Some(vec![2]);
    }
    let event = tokio::time::timeout(Duration::from_millis(100), next.next())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(event, SessionEvent::Completed(Ok(Some(SelectionData::Image(bytes)))) if bytes == [2])
    );
    assert_eq!(port.observation.lock().unwrap().windows, [99]);
    assert!(!port
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|line| line == "escape"));
    drop(session);
    actor.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn draining_failure_does_not_complete_or_fail_a_concurrent_text_close() {
    let port = RecordingPort::default();
    let (session, actor) = port.start();
    let image = open(&session, &port, 1);
    port.recorded("post", 1).await;
    port.observation.lock().unwrap().windows = vec![42];
    port.recorded("opened:1", 1).await;
    {
        let mut observed = port.observation.lock().unwrap();
        observed.count = 1;
        observed.image = Some(vec![1]);
    }
    assert!(matches!(
        image.await.unwrap().unwrap(),
        Some(SelectionData::Image(_))
    ));
    let mut text = session.open(2, CaptureKind::Text).unwrap();
    port.recorded("text:posted:2", 1).await;
    let closing = close(&session, 3);
    port.recorded("selection-session: generation=3 text close requested", 1)
        .await;
    port.observation.lock().unwrap().observe_error = Some("observe failed".into());
    port.recorded(
        "ERROR selection-session: generation=3 error=observe failed",
        1,
    )
    .await;
    assert!(!closing.is_finished());
    assert!(matches!(
        text.0.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    port.observation.lock().unwrap().count = 3;
    assert!(matches!(
        text.next().await.unwrap(),
        SessionEvent::Completed(Ok(None))
    ));
    closing.await.unwrap().unwrap();
    drop(session);
    actor.await.unwrap();
}
