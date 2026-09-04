@main
struct AudioStatsTest {
    static func main() {
        var microphone = AudioStats()
        microphone.recordBuffer(frameCount: 1_024)
        microphone.recordBuffer(frameCount: 512)
        microphone.recordAppend()
        microphone.recordNoSpeechRestart()
        microphone.recordVoiceActivity(
            noiseFloorRms: 0.0004,
            startRmsThreshold: 0.0024,
            sustainRmsThreshold: 0.001
        )
        microphone.recordVolume(
            AudioVolumeMeasurement(peak: 0.42, rms: 0.03, sampleCount: 100)
        )

        var speaker = AudioStats()
        speaker.recordBuffer(frameCount: 2_048)
        speaker.recordAppend()
        speaker.recordVolume(
            AudioVolumeMeasurement(peak: 0.8, rms: 0.2, sampleCount: 50)
        )

        let snapshot = AudioStatsSnapshot(microphone: microphone, speaker: speaker)
        let sources = Set([AudioSource.microphone, AudioSource.speaker])
        assert(
            audioStatsLine(sources: sources, snapshot: snapshot)
                == "microphone buffers=2 frames=1536 appends=1 noSpeechRestarts=1 peak=0.4200 rms=0.0300 floor=0.0004 start=0.0024 sustain=0.0010 speaker buffers=1 frames=2048 appends=1 noSpeechRestarts=0 peak=0.8000 rms=0.2000 floor=0.0000 start=0.0020 sustain=0.0008"
        )
        assert(missingAudioSources(sources: sources, snapshot: snapshot).isEmpty)

        microphone.recordVolume(
            AudioVolumeMeasurement(peak: 0.8, rms: 0.1, sampleCount: 1)
        )
        let expectedRms = (0.03 * 0.03 * 100 + 0.1 * 0.1) / 101
        assert(abs(microphone.rms * microphone.rms - expectedRms) < 0.000_000_001)
        microphone.resetVolumeWindow()
        assert(microphone.peak == 0)
        assert(microphone.rms == 0)

        let microphoneOnly = Set([AudioSource.microphone])
        let emptySnapshot = AudioStatsSnapshot(microphone: AudioStats(), speaker: AudioStats())
        assert(
            missingAudioSources(sources: microphoneOnly, snapshot: emptySnapshot)
                == [.microphone]
        )

        var inputFailureTracker = AudioInputFailureTracker()
        for _ in 0..<(AudioInputFailureTracker.maximumConsecutiveFailures - 1) {
            assert(!inputFailureTracker.recordFailure())
        }
        assert(inputFailureTracker.recordFailure())
        assert(
            inputFailureTracker.consecutiveFailures
                == AudioInputFailureTracker.maximumConsecutiveFailures
        )
        inputFailureTracker.recordSuccessfulBuffer()
        assert(inputFailureTracker.consecutiveFailures == 0)
        assert(!inputFailureTracker.recordFailure())

        try! testAudioConversion()
        testAudioScaling()
        try! testAudioBufferCopy()
        try! testDebugInputWav()
        try! testAppendedAudioDump()
        testVoiceActivity()
        testRecognitionState()
    }
}
