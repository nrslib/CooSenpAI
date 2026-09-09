func testMusicGate() {
    let musicResult = SpeakerMusicClassification(
        musicConfidence: 0.90,
        speechConfidence: 0.05
    )
    assert(SpeakerMusicGatePolicy.decision(for: musicResult) == .suppress)

    let speechResult = SpeakerMusicClassification(
        musicConfidence: 0.05,
        speechConfidence: 0.90
    )
    assert(SpeakerMusicGatePolicy.decision(for: speechResult) == .pass)

    let mixedResult = SpeakerMusicClassification(
        musicConfidence: 0.90,
        speechConfidence: 0.90
    )
    assert(SpeakerMusicGatePolicy.decision(for: mixedResult) == .pass)
}
