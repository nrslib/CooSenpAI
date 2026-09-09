use coosenpai_core::onboarding::TutorialStep;

pub(crate) fn renderer_ready(attempt: u32, generation: u64, records: usize, setup: bool) -> String {
    format!("吹き出しrendererの初期化を確認しsnapshotを配信しました: attempt={attempt} generation={generation} records={records} setup={setup}")
}

pub(crate) fn chat_response(entry_id: &str, delay_ms: u128) -> String {
    format!(
        "チュートリアルのチャット返答を表示しました: entry-id={entry_id} read-delay-ms={delay_ms}"
    )
}

pub(crate) fn response_read(step: TutorialStep) -> String {
    if step == TutorialStep::Chat {
        "チュートリアルのチャット返答を読み終え、コピー練習へ進みました".into()
    } else {
        format!("チュートリアルの返答を読み終え、次の案内へ進みました: step={step:?}")
    }
}

pub(crate) fn selection_observed(pids: &[i32]) -> String {
    format!("範囲選択: 段階=selection-observed pids={pids:?}")
}

pub(crate) fn selection_closed(image: bool) -> String {
    format!("範囲選択: 段階=selection-closed image={image}")
}

pub(crate) fn selection_complete(image: Option<&[u8]>) -> String {
    format!(
        "範囲選択: 段階=complete image={} bytes={}",
        image.is_some(),
        image.map_or(0, <[u8]>::len)
    )
}

