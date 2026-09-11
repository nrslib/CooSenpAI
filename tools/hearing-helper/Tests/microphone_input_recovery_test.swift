import Foundation
import AVFoundation

func testMicrophoneInputRecovery() {
    testMicrophoneInputResetPreservesSpeaker()
    testBackgroundSourceFailureStopsMicrophoneOnMain()
    testOldSourceFailureDoesNotStopReplacementMicrophone()
    let queue = DispatchQueue(label: "microphone-recovery-test")
    let restarted = DispatchSemaphore(value: 0)
    var restartCount = 0
    var recovery: MicrophoneInputRecovery!
    var current: MicrophoneInputGeneration!
    queue.sync {
        recovery = MicrophoneInputRecovery(queue: queue, delay: .milliseconds(10)) {
            restartCount += 1
            current = recovery.begin()
            restarted.signal()
        }
        current = recovery.begin()
        recovery.requestRestart(for: current)
        recovery.requestRestart(for: current)
    }
    assert(restarted.wait(timeout: .now() + 2) == .success)
    queue.sync { assert(restartCount == 1) }

    var stale: MicrophoneInputGeneration!
    queue.sync {
        stale = current
        recovery.requestRestart(for: current)
        recovery.stop()
        assert(!stale.isValid)
    }
    assert(restarted.wait(timeout: .now() + 0.05) == .timedOut)
    queue.sync {
        current = recovery.begin()
        recovery.requestRestart(for: stale)
        assert(current.isValid)
    }
    assert(restarted.wait(timeout: .now() + 0.05) == .timedOut)

    // Work enqueued by the old audio tap must not reach recognition after a rebuild.
    let audioQueue = DispatchQueue(label: "microphone-old-buffer-test")
    let releaseAudio = DispatchSemaphore(value: 0)
    let audioFinished = DispatchSemaphore(value: 0)
    var appended = false
    var old: MicrophoneInputGeneration!
    queue.sync { old = current }
    let captured = old!
    audioQueue.async {
        releaseAudio.wait()
        if captured.isValid { appended = true }
        audioFinished.signal()
    }
    queue.sync { current = recovery.begin() }
    releaseAudio.signal()
    assert(audioFinished.wait(timeout: .now() + 2) == .success)
    assert(!appended)
    queue.sync { recovery.requestRestart(for: current) }
    assert(restarted.wait(timeout: .now() + 2) == .success)
    queue.sync {
        assert(restartCount == 2)
        recovery.stop()
    }
}

private func testMicrophoneInputResetPreservesSpeaker() {
    var controller = RecognitionSegmentController<String, String, String>(
        pendingCapacityNanoseconds: 10_000, preRollCapacityNanoseconds: 10_000
    )
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
    buffer.frameLength = 1
    let pending = PendingAudioBuffer(buffer: buffer, rms: 0.1, timestamp: 1)
    var microphoneGeneration = 0
    for source in AudioSource.allCases {
        let generation = controller.reserveGeneration(for: source)
        if source == .microphone { microphoneGeneration = generation }
        assert(controller.install(source: source, request: source.rawValue,
                                  task: source.rawValue, recognizer: source.rawValue,
                                  generation: generation, sourceIsActive: true))
        _ = controller.appendPending(pending, for: source, durationNanoseconds: 1)
        controller.appendPreRoll(pending, for: source, durationNanoseconds: 1)
    }
    let removed = controller.resetInput(for: .microphone)
    assert(removed?.source == .microphone)
    assert(controller.currentRequest(for: .microphone) == nil)
    assert(!controller.isCurrentState(.microphone, microphoneGeneration))
    assert(!controller.install(source: .microphone, request: "stale", task: "stale",
                               recognizer: "stale", generation: microphoneGeneration,
                               sourceIsActive: true))
    assert(!controller.hasPendingAudio(for: .microphone))
    assert(controller.takePreRoll(for: .microphone).isEmpty)
    assert(controller.currentRequest(for: .speaker) == AudioSource.speaker.rawValue)
    assert(controller.hasPendingAudio(for: .speaker))
    assert(controller.takePreRoll(for: .speaker).count == 1)
}

private final class SourceFailureAppendTarget: AudioBufferAppendTarget {
    var appendCount = 0

    func append(_ buffer: AVAudioPCMBuffer, rms: Double) {
        appendCount += 1
    }
}

private func testBackgroundSourceFailureStopsMicrophoneOnMain() {
    let session = HearingSession(
        locale: Locale(identifier: "ja-JP"), inputDevice: "default",
        sources: [.microphone, .speaker], debugInputWavPath: nil,
        debugDumpAppendedPath: nil, debugRequestAuth: false, speakerBackend: .screenCaptureKit
    )
    let failureQueue = DispatchQueue(label: "hearing-background-source-failure")
    let submitted = DispatchSemaphore(value: 0)
    var mainQueueDrained = false
    failureQueue.async {
        session.disableSource(.microphone, kind: "test", message: "background failure")
        DispatchQueue.main.async { mainQueueDrained = true }
        submitted.signal()
    }
    // A synchronous dispatch to main would deadlock before this signal.
    assert(submitted.wait(timeout: .now() + 2) == .success)
    let deadline = Date(timeIntervalSinceNow: 2)
    while !mainQueueDrained && Date() < deadline {
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01))
    }
    assert(mainQueueDrained)
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
    buffer.frameLength = 1
    buffer.floatChannelData![0][0] = 0.25
    let microphone = SourceFailureAppendTarget()
    let speaker = SourceFailureAppendTarget()
    session.processReceivedAudioBuffer(buffer, for: .microphone, frameCount: 1,
                                       appendTo: microphone)
    session.processReceivedAudioBuffer(buffer, for: .speaker, frameCount: 1,
                                       appendTo: speaker)
    assert(microphone.appendCount == 0)
    assert(speaker.appendCount == 1)
}

private func testOldSourceFailureDoesNotStopReplacementMicrophone() {
    let session = HearingSession(
        locale: Locale(identifier: "ja-JP"), inputDevice: "default",
        sources: [.microphone, .speaker], debugInputWavPath: nil,
        debugDumpAppendedPath: nil, debugRequestAuth: false, speakerBackend: .screenCaptureKit
    )
    let oldGeneration = session.beginMicrophoneInputGeneration()
    let audioQueue = DispatchQueue(label: "hearing-stale-source-failure")
    let processingStarted = DispatchSemaphore(value: 0)
    let continueProcessing = DispatchSemaphore(value: 0)
    var mainQueueDrained = false
    audioQueue.async {
        processingStarted.signal()
        continueProcessing.wait()
        session.disableSource(.microphone, kind: "test", message: "old input failure")
        DispatchQueue.main.async { mainQueueDrained = true }
    }
    assert(processingStarted.wait(timeout: .now() + 2) == .success)
    session.stopMicrophoneInput()
    assert(!oldGeneration.isValid)
    continueProcessing.signal()
    // Rebuild drains old processing before installing its replacement generation.
    audioQueue.sync {}
    let replacement = session.beginMicrophoneInputGeneration()
    let deadline = Date(timeIntervalSinceNow: 2)
    while !mainQueueDrained && Date() < deadline {
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.01))
    }
    assert(mainQueueDrained)
    assert(replacement.isValid)
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
    buffer.frameLength = 1
    buffer.floatChannelData![0][0] = 0.25
    let microphone = SourceFailureAppendTarget()
    session.processReceivedAudioBuffer(buffer, for: .microphone, frameCount: 1,
                                       appendTo: microphone)
    assert(microphone.appendCount == 1)
    // A failure from the replacement still disables it normally.
    session.disableSource(.microphone, kind: "test", message: "current input failure")
    assert(!replacement.isValid)
    session.processReceivedAudioBuffer(buffer, for: .microphone, frameCount: 1,
                                       appendTo: microphone)
    assert(microphone.appendCount == 1)
}
