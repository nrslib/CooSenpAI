private let testAudioBufferDurationNanoseconds: UInt64 = 10

private func configurationForStateTests(
    maximumSegmentNanoseconds: UInt64
) -> VoiceActivityConfiguration {
    VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 0,
        startNoiseMultiplier: 1,
        sustainNoiseMultiplier: 1,
        minimumStartRmsThreshold: 0.01,
        minimumSustainRmsThreshold: 0.005,
        movingRmsWindowNanoseconds: 10,
        maximumSegmentNanoseconds: maximumSegmentNanoseconds,
        trailingNanoseconds: 1_200,
        startAttackNanoseconds: 0,
        rearmQuietNanoseconds: 0
    )
}

private func observe(
    _ detector: inout VoiceActivityDetector,
    rms: Double,
    at timestamp: UInt64,
    durationNanoseconds: UInt64 = testAudioBufferDurationNanoseconds
) -> VoiceActivityAction {
    detector.observe(
        rms: rms,
        durationNanoseconds: durationNanoseconds,
        at: timestamp
    )
}

private func testVoiceActivityStateTransitions() {
    var detector = VoiceActivityDetector(
        configuration: configurationForStateTests(maximumSegmentNanoseconds: 10_000)
    )

    assert(observe(&detector, rms: 0.009, at: 0) == .wait)
    assert(detector.phase == .waiting)
    assert(observe(&detector, rms: 0.011, at: 100) == .start)
    assert(detector.phase == .speaking(startedAt: 100, lastSpeechAt: 100))

    assert(observe(&detector, rms: 0.007, at: 500) == .append)
    assert(detector.phase == .speaking(startedAt: 100, lastSpeechAt: 500))
    assert(observe(&detector, rms: 0.004, at: 1_000) == .append)
    assert(detector.phase == .speaking(startedAt: 100, lastSpeechAt: 500))
    assert(observe(&detector, rms: 0.004, at: 1_700) == .appendAndFinish(.trailing))
    assert(detector.phase == .finishing)
    assert(observe(&detector, rms: 0.5, at: 1_800) == .bufferForNextSegment)
    assert(detector.phase == .pending)
    assert(observe(&detector, rms: 0.004, at: 1_900) == .bufferForNextSegment)

    detector.finishSegment(at: 1_900)
    assert(detector.phase == .rearming(quietSince: 1_900))
    assert(observe(&detector, rms: 0.007, at: 2_000) == .wait)
    assert(observe(&detector, rms: 0.004, at: 2_100) == .wait)
    assert(observe(&detector, rms: 0.011, at: 2_200) == .start)
}

private func testNoiseFloorAndThresholds() {
    let configuration = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 0.5,
        startNoiseMultiplier: 6,
        sustainNoiseMultiplier: 2.5,
        minimumStartRmsThreshold: 0.002,
        minimumSustainRmsThreshold: 0.0008,
        movingRmsWindowNanoseconds: 150,
        maximumSegmentNanoseconds: 15_000,
        trailingNanoseconds: 1_200,
        startAttackNanoseconds: 150,
        maximumNoiseFloorRiseFractionPerSecond: 1_000_000_000
    )
    var detector = VoiceActivityDetector(configuration: configuration)

    assert(observe(&detector, rms: 0.0004, at: 0) == .wait)
    assert(detector.noiseFloorRms == 0)
    assert(observe(&detector, rms: 0.0004, at: 10) == .wait)
    assert(detector.noiseFloorRms == 0)
    assert(detector.startRmsThreshold == 0.002)
    assert(detector.sustainRmsThreshold == 0.0008)

    for index in 0..<30 {
        assert(observe(&detector, rms: 0.0006, at: UInt64(index + 2) * 10) == .wait)
    }
    assert(abs(detector.noiseFloorRms - 0.0006) < 0.000_000_01)
    assert(abs(detector.startRmsThreshold - 0.0036) < 0.000_001)
    assert(abs(detector.sustainRmsThreshold - 0.0015) < 0.000_001)

    let floorBeforeSpeech = detector.noiseFloorRms
    var speechStarted = false
    for index in 0..<30 {
        let action = observe(
            &detector,
            rms: 0.01,
            at: UInt64(index) * 10 + 320
        )
        speechStarted = speechStarted || action == .start
    }
    assert(speechStarted)
    assert(detector.noiseFloorRms == floorBeforeSpeech)
    assert(observe(&detector, rms: 0.01, at: 630) == .append)
    assert(detector.noiseFloorRms == floorBeforeSpeech)
}

private func testNoiseFloorRequiresAQuietMovingWindow() {
    let configuration = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 1,
        startNoiseMultiplier: 6,
        sustainNoiseMultiplier: 2.5,
        minimumStartRmsThreshold: 0.002,
        minimumSustainRmsThreshold: 0.0008,
        movingRmsWindowNanoseconds: 150,
        maximumSegmentNanoseconds: 15_000,
        trailingNanoseconds: 1_200,
        maximumNoiseFloorRiseFractionPerSecond: 1_000_000_000
    )
    var detector = VoiceActivityDetector(configuration: configuration)

    for index in 0..<15 {
        assert(observe(&detector, rms: 0.0004, at: UInt64(index) * 10) == .wait)
    }
    let floorBeforeLoudWaitingInput = detector.noiseFloorRms

    for index in 0..<15 {
        assert(
            observe(
                &detector,
                rms: 0.0015,
                at: UInt64(index + 15) * 10
            ) == .wait
        )
    }
    assert(detector.noiseFloorRms == floorBeforeLoudWaitingInput)
    assert(detector.phase == .waiting)
}

private func testNoiseFloorRiseRateIsLimited() {
    let configuration = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 1,
        startNoiseMultiplier: 6,
        sustainNoiseMultiplier: 2.5,
        minimumStartRmsThreshold: 0.002,
        minimumSustainRmsThreshold: 0.0008,
        movingRmsWindowNanoseconds: 1_000_000_000,
        maximumSegmentNanoseconds: 15_000_000_000,
        trailingNanoseconds: 1_200_000_000,
        maximumNoiseFloorRiseFractionPerSecond: 0.2
    )
    var detector = VoiceActivityDetector(configuration: configuration)

    assert(
        detector.observe(
            rms: 0.0004,
            durationNanoseconds: 1_000_000_000,
            at: 0
        ) == .wait
    )
    assert(abs(detector.noiseFloorRms - 0.00016) < 0.000_000_001)
    assert(
        detector.observe(
            rms: 0.0004,
            durationNanoseconds: 1_000_000_000,
            at: 1_000_000_000
        ) == .wait
    )
    assert(abs(detector.noiseFloorRms - 0.00032) < 0.000_000_001)
}

private func testThresholdsHaveAnUpperBound() {
    let configuration = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 1,
        startNoiseMultiplier: 6,
        sustainNoiseMultiplier: 1,
        minimumStartRmsThreshold: 0.004,
        minimumSustainRmsThreshold: 0.01,
        movingRmsWindowNanoseconds: 10,
        maximumSegmentNanoseconds: 15_000,
        trailingNanoseconds: 1_200,
        maximumStartRmsThreshold: 0.02,
        maximumSustainRmsThreshold: 0.01,
        maximumNoiseFloorRiseFractionPerSecond: 1_000_000_000
    )
    var detector = VoiceActivityDetector(configuration: configuration)

    assert(observe(&detector, rms: 0.0035, at: 0) == .wait)
    assert(abs(detector.noiseFloorRms - 0.0035) < 0.000_000_001)
    assert(detector.startRmsThreshold == 0.02)
    assert(detector.sustainRmsThreshold == 0.01)
}

private func testMovingRmsWindow() {
    var window = MovingRmsWindow(capacityNanoseconds: 150)
    window.append(rms: 1, durationNanoseconds: 100)
    window.append(rms: 0, durationNanoseconds: 100)
    assert(abs(window.rms - (1.0 / 3.0).squareRoot()) < 0.000_000_001)
}

private func testMaximumSegmentForceClose() {
    var detector = VoiceActivityDetector(
        configuration: configurationForStateTests(maximumSegmentNanoseconds: 100)
    )
    assert(observe(&detector, rms: 0.011, at: 0) == .start)
    assert(observe(&detector, rms: 0.02, at: 50) == .append)
    assert(observe(&detector, rms: 0.02, at: 100) == .appendAndFinish(.maximum))
    assert(detector.phase == .finishing)
}

private func testSteadyNoiseDoesNotReachMaximumSegment() {
    let configuration = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 0,
        startNoiseMultiplier: 6,
        sustainNoiseMultiplier: 2.5,
        minimumStartRmsThreshold: 0.002,
        minimumSustainRmsThreshold: 0.0008,
        movingRmsWindowNanoseconds: 10,
        maximumSegmentNanoseconds: 10_000,
        trailingNanoseconds: 1_200,
        startAttackNanoseconds: 0,
        steadyNoiseWindowNanoseconds: 50,
        steadyNoiseDurationNanoseconds: 50,
        steadyNoiseMaximumCoefficientOfVariation: 0.01,
        steadyNoiseMaximumStartRatio: 2
    )
    var detector = VoiceActivityDetector(configuration: configuration)
    assert(observe(&detector, rms: 0.003, at: 0) == .start)

    var closeReason: VoiceActivityFinishReason?
    for index in 1...20 {
        let action = observe(
            &detector,
            rms: 0.003,
            at: UInt64(index) * 10
        )
        if case let .appendAndFinish(reason) = action {
            closeReason = reason
            break
        }
    }
    assert(closeReason == .steadyNoise)
    assert(detector.phase == .finishing)
}

private func testStandardSteadyNoiseRearmsForSpeech() {
    var detector = VoiceActivityDetector(configuration: .standard)
    let duration: UInt64 = 10_000_000
    var timestamp: UInt64 = 0
    for _ in 0..<200 {
        assert(
            observe(
                &detector,
                rms: 0.0003,
                at: timestamp,
                durationNanoseconds: duration
            ) == .wait
        )
        timestamp += duration
    }

    let floorBeforeSteadyNoise = detector.noiseFloorRms
    var closeReason: VoiceActivityFinishReason?
    for _ in 0..<2_000 {
        let action = observe(
            &detector,
            rms: 0.004,
            at: timestamp,
            durationNanoseconds: duration
        )
        if case let .appendAndFinish(reason) = action {
            closeReason = reason
            timestamp += duration
            break
        }
        timestamp += duration
    }
    assert(closeReason == .steadyNoise)
    assert(detector.noiseFloorRms > floorBeforeSteadyNoise)
    detector.finishSegment(at: timestamp)

    var waiting = false
    for _ in 0..<60 {
        assert(
            observe(
                &detector,
                rms: 0.004,
                at: timestamp,
                durationNanoseconds: duration
            ) == .wait
        )
        waiting = waiting || detector.phase == .waiting
        timestamp += duration
    }
    assert(waiting)

    var speechStarted = false
    for _ in 0..<30 {
        let action = observe(
            &detector,
            rms: 0.012,
            at: timestamp,
            durationNanoseconds: duration
        )
        speechStarted = speechStarted || action == .start
        timestamp += duration
    }
    assert(speechStarted)
}

private func testNoiseDoesNotStartRecognition() {
    var detector = VoiceActivityDetector(configuration: .standard)
    var timestamp: UInt64 = 0
    for _ in 0..<200 {
        assert(
            observe(
                &detector,
                rms: 0.0004,
                at: timestamp,
                durationNanoseconds: 10_000_000
            ) == .wait
        )
        timestamp += 10_000_000
    }
    assert(detector.phase == .waiting)
    assert(detector.noiseFloorRms > 0)
    assert(detector.noiseFloorRms < 0.0004)
    assert(detector.startRmsThreshold > 0.0004)
}

private func testSingleNoiseImpulseDoesNotStartRecognition() {
    var detector = VoiceActivityDetector(configuration: .standard)
    var timestamp: UInt64 = 0
    for _ in 0..<200 {
        assert(
            observe(
                &detector,
                rms: 0.0004,
                at: timestamp,
                durationNanoseconds: 10_000_000
            ) == .wait
        )
        timestamp += 10_000_000
    }

    assert(
        observe(
            &detector,
            rms: 0.01,
            at: timestamp,
            durationNanoseconds: 10_000_000
        ) == .wait
    )
    timestamp += 10_000_000
    for _ in 0..<40 {
        assert(
            observe(
                &detector,
                rms: 0.0004,
                at: timestamp,
                durationNanoseconds: 10_000_000
            ) == .wait
        )
        timestamp += 10_000_000
    }
    assert(detector.phase == .waiting)
}

private func testRearmRequiresQuietInput() {
    var detector = VoiceActivityDetector(
        configuration: VoiceActivityConfiguration(
            noiseFloorAdaptationRate: 0,
            startNoiseMultiplier: 1,
            sustainNoiseMultiplier: 1,
            minimumStartRmsThreshold: 0.01,
            minimumSustainRmsThreshold: 0.005,
            movingRmsWindowNanoseconds: 10,
            maximumSegmentNanoseconds: 10_000,
            trailingNanoseconds: 1_200,
            startAttackNanoseconds: 30,
            rearmQuietNanoseconds: 100
        )
    )
    assert(
        observe(
            &detector,
            rms: 0.011,
            at: 0,
            durationNanoseconds: 30
        ) == .start
    )
    detector.finishSegment(at: 30)
    assert(observe(&detector, rms: 0.02, at: 40) == .wait)
    assert(detector.phase == .rearming(quietSince: nil))
    assert(observe(&detector, rms: 0.004, at: 50) == .wait)
    assert(detector.phase == .rearming(quietSince: 50))
    assert(observe(&detector, rms: 0.004, at: 140) == .wait)
    assert(detector.phase == .waiting)
    assert(observe(&detector, rms: 0.004, at: 150) == .wait)
    assert(detector.phase == .waiting)
    assert(observe(&detector, rms: 0.02, at: 160) == .wait)
    assert(observe(&detector, rms: 0.02, at: 190) == .start)
}

private func testRecognizerFinalCanRearmImmediately() {
    var detector = VoiceActivityDetector(
        configuration: VoiceActivityConfiguration(
            noiseFloorAdaptationRate: 0,
            startNoiseMultiplier: 1,
            sustainNoiseMultiplier: 1,
            minimumStartRmsThreshold: 0.01,
            minimumSustainRmsThreshold: 0.005,
            movingRmsWindowNanoseconds: 10,
            maximumSegmentNanoseconds: 10_000,
            trailingNanoseconds: 1_200,
            startAttackNanoseconds: 0,
            rearmQuietNanoseconds: 400
        )
    )
    assert(observe(&detector, rms: 0.02, at: 0) == .start)
    detector.finishSegment(at: 10, rearmImmediately: true)
    assert(detector.phase == .waiting)
    assert(observe(&detector, rms: 0.02, at: 20) == .start)
}

private func testStartAttackRequiresContinuousInput() {
    var detector = VoiceActivityDetector(
        configuration: VoiceActivityConfiguration(
            noiseFloorAdaptationRate: 0,
            startNoiseMultiplier: 1,
            sustainNoiseMultiplier: 1,
            minimumStartRmsThreshold: 0.01,
            minimumSustainRmsThreshold: 0.005,
            movingRmsWindowNanoseconds: 10,
            maximumSegmentNanoseconds: 10_000,
            trailingNanoseconds: 1_200,
            startAttackNanoseconds: 30,
            rearmQuietNanoseconds: 400
        )
    )
    assert(observe(&detector, rms: 0.02, at: 0) == .wait)
    assert(observe(&detector, rms: 0.02, at: 10) == .wait)
    assert(observe(&detector, rms: 0.004, at: 20) == .wait)
    assert(observe(&detector, rms: 0.02, at: 30) == .wait)
    assert(observe(&detector, rms: 0.02, at: 40) == .wait)
    assert(observe(&detector, rms: 0.02, at: 50) == .start)
}

private func testWeakVoiceStaysOpenAcrossSyllableValleys() {
    var detector = VoiceActivityDetector(configuration: .standard)
    var timestamp: UInt64 = 0
    for _ in 0..<200 {
        assert(
            observe(
                &detector,
                rms: 0.0004,
                at: timestamp,
                durationNanoseconds: 10_000_000
            ) == .wait
        )
        timestamp += 10_000_000
    }

    let speechStartedAt = timestamp
    var started = false
    for _ in 0..<20 {
        let action = observe(
            &detector,
            rms: 0.003,
            at: timestamp,
            durationNanoseconds: 20_000_000
        )
        started = started || action == .start
        timestamp += 20_000_000
    }
    assert(started)
    for index in 0..<120 {
        let rms = index % 2 == 0 ? 0.003 : 0.0004
        let action = observe(
            &detector,
            rms: rms,
            at: timestamp,
            durationNanoseconds: 20_000_000
        )
        if case .appendAndFinish = action {
            assertionFailure("音節間の谷で区間を閉じてはいけません")
        }
        timestamp += 20_000_000
    }
    guard case let .speaking(startedAt, lastSpeechAt) = detector.phase else {
        assertionFailure("弱い声の音節間で区間が閉じています")
        return
    }
    assert(startedAt >= speechStartedAt)
    assert(lastSpeechAt == timestamp - 20_000_000)
}

private func testPreRollEvictsByAudioTime() {
    var preRoll = RollingAudioWindow<Int>(capacityNanoseconds: 300)
    preRoll.append(1, durationNanoseconds: 100)
    preRoll.append(2, durationNanoseconds: 100)
    preRoll.append(3, durationNanoseconds: 100)
    preRoll.append(4, durationNanoseconds: 100)
    assert(preRoll.removeAll() == [2, 3, 4])
}

func testVoiceActivity() {
    testVoiceActivityStateTransitions()
    testNoiseFloorAndThresholds()
    testNoiseFloorRequiresAQuietMovingWindow()
    testNoiseFloorRiseRateIsLimited()
    testThresholdsHaveAnUpperBound()
    testMovingRmsWindow()
    testMaximumSegmentForceClose()
    testSteadyNoiseDoesNotReachMaximumSegment()
    testStandardSteadyNoiseRearmsForSpeech()
    testNoiseDoesNotStartRecognition()
    testSingleNoiseImpulseDoesNotStartRecognition()
    testRearmRequiresQuietInput()
    testRecognizerFinalCanRearmImmediately()
    testStartAttackRequiresContinuousInput()
    testWeakVoiceStaysOpenAcrossSyllableValleys()
    testPreRollEvictsByAudioTime()

    var pending = PendingAudioWindow<Int>(capacityNanoseconds: 4_000)
    assert(pending.append(1, durationNanoseconds: 1_000))
    assert(pending.append(2, durationNanoseconds: 1_000))
    assert(pending.append(3, durationNanoseconds: 1_000))
    assert(pending.append(4, durationNanoseconds: 1_000))
    assert(!pending.append(5, durationNanoseconds: 1_000))
    assert(pending.removeAll() == [1, 2, 3, 4])

}
