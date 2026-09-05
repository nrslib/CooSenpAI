import AVFoundation
import AudioToolbox
import CoreAudio
import CoreMedia
import Foundation
@preconcurrency import ScreenCaptureKit
import Speech

private let outputLock = NSLock()
private let noSpeechDetectedErrorCode = 1110
private let debugInputPlaybackRate = 1.0

private func monotonicNanoseconds() -> UInt64 {
    DispatchTime.now().uptimeNanoseconds
}

private struct EnqueuedAudioBufferAppendTarget: AudioBufferAppendTarget {
    let enqueue: (AVAudioPCMBuffer, Double) -> Void

    func append(_ buffer: AVAudioPCMBuffer, rms: Double) {
        enqueue(buffer, rms)
    }
}

private func emit(_ value: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: value) else { return }
    outputLock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    outputLock.unlock()
}

private func emitStderr(_ message: String) {
    outputLock.lock()
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    outputLock.unlock()
}

private func permissionName(_ status: AVAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not-determined"
    case .authorized: return "granted"
    case .denied: return "denied"
    case .restricted: return "restricted"
    @unknown default: return "unavailable"
    }
}

private func permissionName(_ status: SFSpeechRecognizerAuthorizationStatus) -> String {
    switch status {
    case .notDetermined: return "not-determined"
    case .authorized: return "granted"
    case .denied: return "denied"
    case .restricted: return "restricted"
    @unknown default: return "unavailable"
    }
}

private func speechAuthorizationStatusName(
    _ status: SFSpeechRecognizerAuthorizationStatus
) -> String {
    switch status {
    case .notDetermined: return "notDetermined"
    case .authorized: return "authorized"
    case .denied: return "denied"
    case .restricted: return "restricted"
    @unknown default: return "unknown"
    }
}

private enum RecognitionCancellationTimeoutStage {
    case cancellationTimeout
    case graceExpired
}

final class HearingSession: NSObject, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    private let locale: Locale
    private let inputDevice: String
    private let sources: Set<AudioSource>
    private let debugInputWavPath: String?
    private let debugDumpAppendedPath: String?
    private let debugRequestAuth: Bool
    private let sourceLock = NSLock()
    private let audioProcessingQueue = DispatchQueue(
        label: "dev.nrslib.coosenpai.hearing.processing",
        qos: .userInitiated
    )
    private let audioProcessingQueueKey = DispatchSpecificKey<Void>()
    private var recognitionStates: RecognitionSegmentController<
        SFSpeechAudioBufferRecognitionRequest,
        SFSpeechRecognitionTask,
        SFSpeechRecognizer
    >
    private var restartTrackers: [AudioSource: RecognitionRestartTracker] = [:]
    private var cancellationTimeoutRecoveryTrackers: [
        AudioSource: RecognitionCancellationTimeoutRecoveryTracker
    ] = [:]
    private var recognitionTimeoutWorkItems: [AudioSource: DispatchWorkItem] = [:]
    private var recognitionCancellationWorkItems: [AudioSource: DispatchWorkItem] = [:]
    private var pendingDrainWorkItems: [AudioSource: DispatchWorkItem] = [:]
    private var voiceActivity: [AudioSource: VoiceActivityDetector] = [
        .microphone: VoiceActivityDetector(configuration: .standard),
        .speaker: VoiceActivityDetector(configuration: .standard),
    ]
    private var recognizers: [AudioSource: SFSpeechRecognizer] = [:]
    private var sourceAvailability: AudioSourceAvailability
    private var audioEngine: AVAudioEngine?
    private var debugInputPlayer: DebugInputWavPlayer?
    private var appendedAudioDump: AppendedAudioDump?
    private var debugDumpOnlyRequest: SFSpeechAudioBufferRecognitionRequest?
    private var debugDumpOnlyGeneration: Int?
    private var speakerStream: SCStream?
    private var microphoneStarted = false
    private var speakerStarted = false
    private var microphoneAuthorization: AVAuthorizationStatus?
    private var recognitionAuthorization: SFSpeechRecognizerAuthorizationStatus?
    private var terminal = false
    private var readySent = false
    private var tapInstalled = false
    private var microphoneStats = AudioStats()
    private var speakerStats = AudioStats()
    private var statsTimer: DispatchSourceTimer?
    private var noBufferWarningTimer: DispatchSourceTimer?
    private var reportedReceivedFormats: Set<AudioSource> = []
    private var reportedRequestFormats: Set<AudioSource> = []
    private var reportedAppendFormats: Set<AudioSource> = []
    private var reportedAudioConversionErrors: Set<AudioSource> = []
    private var reportedAudioMonoConversionErrors: Set<AudioSource> = []
    private var reportedAudioBufferCopyErrors: Set<AudioSource> = []
    private var reportedAudioVolumeErrors: Set<AudioSource> = []
    private var reportedAudioScaleWarnings: Set<AudioSource> = []
    private var reportedPendingOverflows: Set<AudioSource> = []
    private var audioInputFailureTrackers: [AudioSource: AudioInputFailureTracker] = [
        .microphone: AudioInputFailureTracker(),
        .speaker: AudioInputFailureTracker(),
    ]
    private var pendingSourceDisables: Set<AudioSource> = []
    private var debugInputEnded = false

    init(
        locale: Locale,
        inputDevice: String,
        sources: Set<AudioSource>,
        debugInputWavPath: String?,
        debugDumpAppendedPath: String?,
        debugRequestAuth: Bool
    ) {
        self.locale = locale
        self.inputDevice = inputDevice
        self.sources = sources
        self.debugInputWavPath = debugInputWavPath
        self.debugDumpAppendedPath = debugDumpAppendedPath
        self.debugRequestAuth = debugRequestAuth
        self.recognitionStates = RecognitionSegmentController(
            pendingCapacityNanoseconds: pendingAudioWindowCapacityNanoseconds,
            preRollCapacityNanoseconds: preRollAudioWindowCapacityNanoseconds
        )
        self.sourceAvailability = AudioSourceAvailability(sources: sources)
        audioProcessingQueue.setSpecific(key: audioProcessingQueueKey, value: ())
    }

    private func syncOnAudioProcessingQueue<T>(_ body: () -> T) -> T {
        if DispatchQueue.getSpecific(key: audioProcessingQueueKey) != nil {
            return body()
        }
        return audioProcessingQueue.sync(execute: body)
    }

    func authorizeAndStart() {
        if debugRequestAuth {
            requestSpeechAuthorizationForDebug()
            return
        }
        let speechStatus = SFSpeechRecognizer.authorizationStatus()
        emitStderr("speech-auth status=\(speechAuthorizationStatusName(speechStatus))")
        guard speechStatus == .authorized else {
            if let debugInputWavPath,
               debugDumpAppendedPath != nil,
               sources == Set([AudioSource.microphone]) {
                guard prepareAppendedAudioDump() else { return }
                startDiagnostics()
                microphoneAuthorization = .authorized
                recognitionAuthorization = speechStatus
                startDebugDumpOnlyInput(
                    debugInputWavPath,
                    .authorized,
                    speechStatus
                )
                return
            }
            fail(
                "permission-speech",
                "音声認識の使用が許可されていません: status=\(speechAuthorizationStatusName(speechStatus))"
            )
            return
        }
        guard prepareAppendedAudioDump() else { return }
        startDiagnostics()
        if debugInputWavPath != nil {
            handleMicrophoneAuthorization(.authorized, speechStatus: speechStatus)
        } else if sources.contains(.microphone) {
            requestMicrophone { [weak self] microphone in
                DispatchQueue.main.async {
                    self?.handleMicrophoneAuthorization(microphone, speechStatus: speechStatus)
                }
            }
        } else {
            handleMicrophoneAuthorization(
                AVCaptureDevice.authorizationStatus(for: .audio),
                speechStatus: speechStatus
            )
        }
    }

    private func prepareAppendedAudioDump() -> Bool {
        guard let debugDumpAppendedPath else { return true }
        do {
            appendedAudioDump = try AppendedAudioDump(
                directoryURL: URL(fileURLWithPath: debugDumpAppendedPath, isDirectory: true)
            )
            emitStderr("audio-dump directory=\(debugDumpAppendedPath)")
            return true
        } catch {
            fail(
                "debug-dump",
                "追加音声ダンプを初期化できませんでした: \(errorDetails(error))"
            )
            return false
        }
    }

    private func requestSpeechAuthorizationForDebug() {
        SFSpeechRecognizer.requestAuthorization { [weak self] status in
            DispatchQueue.main.async {
                guard let self, !self.isTerminal() else { return }
                emitStderr(
                    "speech-auth status=\(speechAuthorizationStatusName(status))"
                )
                self.close()
            }
        }
    }

    private func handleMicrophoneAuthorization(
        _ microphone: AVAuthorizationStatus,
        speechStatus: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !isTerminal() else { return }
        microphoneAuthorization = microphone
        if sources.contains(.microphone), debugInputWavPath == nil, microphone != .authorized {
            disableSource(
                .microphone,
                kind: "permission-microphone",
                message: "マイクの使用が許可されていません"
            )
            guard !isTerminal() else { return }
        }
        handleRecognitionAuthorization(microphone, speechStatus)
    }

    private func handleRecognitionAuthorization(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !isTerminal() else { return }
        recognitionAuthorization = recognition
        guard recognition == .authorized else {
            fail("permission-speech", "音声認識の使用が許可されていません")
            return
        }
        if sources.contains(.microphone) {
            if isSourceActive(.microphone) {
                startMicrophone(microphone, recognition)
            }
        }
        if sources.contains(.speaker) {
            if isSourceActive(.speaker) {
                startSpeaker(microphone, recognition)
            }
        }
        if sources.isEmpty {
            fail("arguments", "入力源が指定されていません")
        }
    }

    private func requestMicrophone(_ completion: @escaping @Sendable (AVAuthorizationStatus) -> Void) {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        guard status == .notDetermined else {
            completion(status)
            return
        }
        AVCaptureDevice.requestAccess(for: .audio) { granted in
            completion(granted ? .authorized : .denied)
        }
    }

    private func startMicrophone(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !isTerminal(), isSourceActive(.microphone) else { return }
        if let debugInputWavPath {
            startDebugInput(debugInputWavPath, microphone, recognition)
            return
        }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        if inputDevice != "default" {
            do {
                try selectInputDevice(inputDevice, inputNode: input)
            } catch {
                do {
                    try useDefaultInputDevice(inputNode: input)
                } catch {
                    disableSource(
                        .microphone,
                        kind: "input-device",
                        message: "選択したマイクとシステム既定のマイクを利用できません: \(errorDetails(error))"
                    )
                    return
                }
                emit([
                    "event": "warning",
                    "kind": "input-device-fallback",
                    "message": "選択したマイクを利用できないため、システム既定を使います",
                ])
            }
        }
        audioEngine = engine
        do {
            let format = input.outputFormat(forBus: 0)
            try installAudioTap(on: input, bufferSize: 1_024, format: format) {
                [weak self] buffer, _ in
                self?.receive(buffer, for: .microphone)
            }
            tapInstalled = true
            emitStderr(
                "audio-format microphone tap=\(audioFormatDescription(format)) append=pending"
            )
        } catch {
            disableSource(
                .microphone,
                kind: "audio-microphone",
                message: "マイクの音声タップを設置できませんでした: \(errorDetails(error))"
            )
            return
        }
        do {
            engine.prepare()
            try engine.start()
            microphoneStarted = true
            emitReadyIfPossible(microphone, recognition)
        } catch {
            disableSource(
                .microphone,
                kind: "audio-microphone",
                message: "マイクの音声入力を開始できませんでした: \(errorDetails(error))"
            )
        }
    }

    private func startDebugInput(
        _ path: String,
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        do {
            let player = try DebugInputWavPlayer(
                path: path,
                playbackRate: debugInputPlaybackRate
            )
            debugInputPlayer = player
            emitStderr(
                "audio-debug-input source=\(player.source.rawValue) format=\(audioFormatDescription(player.format)) frames=\(player.frameLength) playbackRate=\(debugInputPlaybackRate)"
            )
            player.start(
                onBuffer: { [weak self] buffer in
                    self?.receive(buffer, for: .microphone)
                },
                onCompletion: { [weak self] result in
                    self?.debugInputDidFinish(result)
                }
            )
            microphoneStarted = true
            emitReadyIfPossible(microphone, recognition)
        } catch {
            disableSource(
                .microphone,
                kind: "debug-input",
                message: "デバッグ用 WAV を開始できませんでした: \(errorDetails(error))"
            )
        }
    }

    private func startDebugDumpOnlyInput(
        _ path: String,
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard appendedAudioDump != nil else {
            fail("debug-dump", "追加音声ダンプが初期化されていません")
            return
        }
        do {
            let player = try DebugInputWavPlayer(
                path: path,
                playbackRate: debugInputPlaybackRate
            )
            let request = SFSpeechAudioBufferRecognitionRequest()
            request.shouldReportPartialResults = true
            request.requiresOnDeviceRecognition = true
            sourceLock.lock()
            guard !terminal, sourceAvailability.isActive(.microphone) else {
                sourceLock.unlock()
                return
            }
            debugDumpOnlyRequest = request
            debugDumpOnlyGeneration = 1
            sourceLock.unlock()
            debugInputPlayer = player
            emitStderr(
                "audio-debug-input mode=dump-only speech-auth=\(speechAuthorizationStatusName(recognition)) source=\(player.source.rawValue) format=\(audioFormatDescription(player.format)) frames=\(player.frameLength) playbackRate=\(debugInputPlaybackRate)"
            )
            player.start(
                onBuffer: { [weak self] buffer in
                    self?.receive(buffer, for: .microphone)
                },
                onCompletion: { [weak self] result in
                    self?.debugInputDidFinish(result)
                }
            )
            microphoneStarted = true
            emitReadyIfPossible(microphone, recognition)
        } catch {
            disableSource(
                .microphone,
                kind: "debug-input",
                message: "デバッグ用 WAV のダンプを開始できませんでした: \(errorDetails(error))"
            )
        }
    }

    private func startSpeaker(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !isTerminal(), isSourceActive(.speaker) else { return }
        Task { [weak self] in
            guard let self else { return }
            guard self.isSourceActive(.speaker) else { return }
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(
                    false,
                    onScreenWindowsOnly: true
                )
                guard let display = content.displays.first else {
                    throw HearingError.noDisplay
                }
                let filter = SCContentFilter(display: display, excludingWindows: [])
                let configuration = SCStreamConfiguration()
                configuration.capturesAudio = true
                configuration.excludesCurrentProcessAudio = true
                configuration.sampleRate = 48_000
                configuration.channelCount = 2
                emitStderr(
                    "audio-format speaker capture=sampleRate=\(configuration.sampleRate) channels=\(configuration.channelCount) commonFormat=unknown-until-first-sample converted-append=mono-float32"
                )
                let stream = SCStream(filter: filter, configuration: configuration, delegate: self)
                try stream.addStreamOutput(
                    self,
                    type: .audio,
                    sampleHandlerQueue: DispatchQueue(label: "dev.nrslib.coosenpai.hearing.audio")
                )
                try await stream.startCapture()
                DispatchQueue.main.async {
                    guard !self.isTerminal(), self.isSourceActive(.speaker) else {
                        Task { try? await stream.stopCapture() }
                        return
                    }
                    self.speakerStream = stream
                    self.speakerStarted = true
                    self.emitReadyIfPossible(microphone, recognition)
                }
            } catch {
                let details = errorDetails(error)
                DispatchQueue.main.async {
                    self.disableSource(
                        .speaker,
                        kind: "screen-capture",
                        message: "スピーカーの音声入力を開始できませんでした: \(details)"
                    )
                }
            }
        }
    }

    private func installAudioTap(
        on inputNode: AVAudioInputNode,
        bufferSize: AVAudioFrameCount,
        format: AVAudioFormat,
        block: @escaping AVAudioNodeTapBlock
    ) throws {
        var error: NSError?
        guard coosenpai_install_audio_tap(inputNode, bufferSize, format, block, &error) else {
            if let error {
                throw error
            }
            throw HearingError.audioTap
        }
    }

    private func emitReadyIfPossible(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !readySent, !isTerminal() else { return }
        let microphoneReady = !sources.contains(.microphone)
            || !isSourceActive(.microphone)
            || microphoneStarted
        let speakerReady = !sources.contains(.speaker)
            || !isSourceActive(.speaker)
            || speakerStarted
        guard microphoneReady && speakerReady else { return }
        readySent = true
        emit([
            "event": "ready",
            "locale": locale.identifier,
            "microphone": permissionName(microphone),
            "recognition": permissionName(recognition),
        ])
    }

    private func speechRecognizer(for source: AudioSource) -> SFSpeechRecognizer? {
        sourceLock.lock()
        if let recognizer = recognizers[source] {
            sourceLock.unlock()
            return recognizer
        }
        sourceLock.unlock()

        guard let created = SFSpeechRecognizer(locale: locale) else { return nil }
        sourceLock.lock()
        guard !terminal, sourceAvailability.isActive(source) else {
            sourceLock.unlock()
            return nil
        }
        if let existing = recognizers[source] {
            sourceLock.unlock()
            return existing
        }
        recognizers[source] = created
        sourceLock.unlock()
        return created
    }

    private func startRecognition(for source: AudioSource) -> Bool {
        guard !isTerminal(), isSourceActive(source) else { return false }
        guard let recognizer = speechRecognizer(for: source), recognizer.isAvailable else {
            disableSource(
                source,
                kind: "recognition-\(source.rawValue)",
                message: "指定したロケールの音声認識は利用できません: \(locale.identifier)"
            )
            return false
        }
        guard recognizer.supportsOnDeviceRecognition else {
            disableSource(
                source,
                kind: "recognition-\(source.rawValue)",
                message: "指定したロケールはオンデバイス音声認識に対応していません: \(locale.identifier)"
            )
            return false
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = true
        let generation: Int
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.currentGeneration(for: source) == nil else {
            sourceLock.unlock()
            return false
        }
        generation = recognitionStates.reserveGeneration(for: source)
        sourceLock.unlock()
        let callbackGate = RecognitionCallbackGate()
        let task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            guard let self else { return }
            callbackGate.enqueue {
                if result?.isFinal == true || error != nil {
                    self.markRecognitionTaskTerminal(source: source, generation: generation)
                }
                self.audioProcessingQueue.async { [weak self] in
                    self?.handleRecognitionResult(
                        result,
                        error: error,
                        source: source,
                        generation: generation
                    )
                }
            }
        }
        sourceLock.lock()
        let sourceIsActiveAtRegistration = sourceAvailability.isActive(source)
        guard !terminal,
              sourceIsActiveAtRegistration,
              recognitionStates.isCurrentGeneration(source, generation) else {
            recognitionStates.retireGeneration(source: source, generation: generation)
            sourceLock.unlock()
            callbackGate.discard()
            cancelRecognitionTask(
                task,
                for: source,
                generation: generation,
                reason: .registrationRejected
            )
            return false
        }
        let installed = recognitionStates.install(
            source: source,
            request: request,
            task: task,
            recognizer: recognizer,
            generation: generation,
            sourceIsActive: sourceIsActiveAtRegistration
        )
        guard installed else {
            recognitionStates.retireGeneration(source: source, generation: generation)
            sourceLock.unlock()
            callbackGate.discard()
            cancelRecognitionTask(
                task,
                for: source,
                generation: generation,
                reason: .registrationRejected
            )
            return false
        }
        let shouldReportRequestFormat = reportedRequestFormats.insert(source).inserted
        sourceLock.unlock()
        emitStderr(
            "recognition-segment-open source=\(source.rawValue) generation=\(generation)"
        )
        if shouldReportRequestFormat {
            emitStderr(
                "audio-format \(source.rawValue) request-native=\(audioFormatDescription(request.nativeAudioFormat))"
            )
        }
        callbackGate.open()
        return true
    }

    private func markRecognitionTaskTerminal(source: AudioSource, generation: Int) {
        syncOnAudioProcessingQueue {
            _ = recognitionStates.markTaskTerminal(source: source, generation: generation)
        }
    }

    private func handleRecognitionResult(
        _ result: SFSpeechRecognitionResult?,
        error: Error?,
        source: AudioSource,
        generation: Int
    ) {
        guard isCurrentRecognitionTask(source: source, generation: generation) else {
            return
        }
        if let result, result.isFinal {
            let text = result.bestTranscription.formattedString
                .trimmingCharacters(in: .whitespacesAndNewlines)
            completeRecognitionTask(
                for: source,
                generation: generation,
                outcome: .success(text: text)
            )
            return
        }
        if currentRecognitionLifecycle(for: source) == .cancelling {
            guard currentRecognitionTaskTerminalArrived(for: source) else { return }
            completeRecognitionTask(
                for: source,
                generation: generation,
                outcome: .cancelled
            )
            return
        }
        guard let error else { return }
        if isNoSpeechError(error) {
            recordNoSpeechRestart(for: source)
            let restartDecision = recordRecognitionRestart(for: source)
            completeRecognitionTask(
                for: source,
                generation: generation,
                outcome: .noSpeech
            )
            if restartDecision?.thresholdReached == true {
                disableSource(
                    source,
                    kind: "recognition-" + source.rawValue,
                    message: "音声認識の再開が異常に繰り返されています: recentRestarts="
                        + String(restartDecision?.recentCount ?? 0)
                        + " windowSeconds=60"
                )
            }
        } else {
            completeRecognitionTask(
                for: source,
                generation: generation,
                outcome: .error(error)
            )
        }
    }

    private func completeRecognitionTask(
        for source: AudioSource,
        generation: Int,
        outcome: RecognitionTaskOutcome
    ) {
        let lifecycle: RecognitionSegmentLifecycle
        let timeoutWorkItem: DispatchWorkItem?
        let cancellationWorkItem: DispatchWorkItem?
        let vadWasSpeaking: Bool
        let segmentCloseReason: RecognitionSegmentCloseReason?
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              let currentLifecycle = recognitionStates.lifecycle(for: source),
              recognitionStates.isCurrentState(source, generation),
              let removedState = recognitionStates.remove(
                  source: source,
                  generation: generation
              )
        else {
            sourceLock.unlock()
            return
        }
        lifecycle = currentLifecycle
        vadWasSpeaking = voiceActivity[source]?.isSpeaking == true
        segmentCloseReason = removedState.closeReason
        timeoutWorkItem = recognitionTimeoutWorkItems.removeValue(forKey: source)
        cancellationWorkItem = recognitionCancellationWorkItems.removeValue(forKey: source)
        cancellationTimeoutRecoveryTrackers[source]?.reset()
        sourceLock.unlock()

        timeoutWorkItem?.cancel()
        cancellationWorkItem?.cancel()
        appendedAudioDump?.close(source: source, generation: generation)
        let terminalOutcome: RecognitionTaskOutcome
        if lifecycle == .cancelling {
            if case .success = outcome {
                terminalOutcome = outcome
            } else {
                terminalOutcome = .cancelled
            }
        } else {
            terminalOutcome = outcome
        }
        let needsSegmentClose = lifecycle == .accepting || lifecycle == .terminal
        switch terminalOutcome {
        case let .success(text):
            if needsSegmentClose {
                emitRecognitionSegmentClose(
                    source: source,
                    generation: generation,
                    reason: .recognizerFinal
                )
            }
            emitStderr(
                "recognition-final-received source=\(source.rawValue) generation=\(generation) chars=\(text.count)"
            )
            if !text.isEmpty {
                emit(["event": "final", "source": source.rawValue, "text": text])
            }
            emitRecognitionTaskFinished(
                source: source,
                generation: generation,
                outcome: "success"
            )
        case .noSpeech:
            if needsSegmentClose {
                emitRecognitionSegmentClose(
                    source: source,
                    generation: generation,
                    reason: .noSpeech
                )
            }
            emitRecognitionTaskFinished(
                source: source,
                generation: generation,
                outcome: "noSpeech"
            )
        case let .error(error):
            if needsSegmentClose {
                emitRecognitionSegmentClose(
                    source: source,
                    generation: generation,
                    reason: .error
                )
            }
            emitRecognitionTaskFinished(
                source: source,
                generation: generation,
                outcome: "error",
                details: errorDetails(error)
            )
            disableSource(
                source,
                kind: "recognition-\(source.rawValue)",
                message: "音声認識に失敗しました: \(errorDetails(error))"
            )
            return
        case .cancelled:
            emitRecognitionTaskFinished(
                source: source,
                generation: generation,
                outcome: "cancelled"
            )
        }

        let rearmImmediately: Bool
        switch terminalOutcome {
        case .success:
            rearmImmediately = shouldRearmRecognitionImmediately(
                lifecycle: lifecycle,
                vadWasSpeaking: vadWasSpeaking,
                closeReason: segmentCloseReason
            )
        case .noSpeech, .error, .cancelled:
            rearmImmediately = false
        }

        let cooldownNanoseconds: UInt64
        switch terminalOutcome {
        case .noSpeech:
            cooldownNanoseconds = RecognitionRestartThrottle.minimumIntervalNanoseconds
        case .success, .error, .cancelled:
            cooldownNanoseconds = 0
        }
        finishCompletedRecognitionSegment(
            for: source,
            cooldownNanoseconds: cooldownNanoseconds,
            rearmImmediately: rearmImmediately
        )
    }

    private func isCurrentRecognitionTask(source: AudioSource, generation: Int) -> Bool {
        sourceLock.lock()
        let isCurrent = !terminal
            && sourceAvailability.isActive(source)
            && recognitionStates.isCurrentState(source, generation)
        sourceLock.unlock()
        return isCurrent
    }

    private func currentRecognitionTaskTerminalArrived(for source: AudioSource) -> Bool {
        sourceLock.lock()
        let terminalArrived = recognitionStates.taskTerminalArrived(for: source)
        sourceLock.unlock()
        return terminalArrived
    }

    private func emitRecognitionTaskFinished(
        source: AudioSource,
        generation: Int,
        outcome: String,
        details: String? = nil
    ) {
        let suffix = details.map { " details=\($0)" } ?? ""
        emitStderr(
            "recognition-task-finished source=\(source.rawValue) generation=\(generation) outcome=\(outcome)\(suffix)"
        )
    }

    private func emitRecognitionSegmentClose(
        source: AudioSource,
        generation: Int,
        reason: RecognitionSegmentCloseReason
    ) {
        emitStderr(
            "recognition-segment-close source=\(source.rawValue) generation=\(generation) reason=\(reason.rawValue)"
        )
    }

    private func cancelRecognitionTask(
        _ task: SFSpeechRecognitionTask,
        for source: AudioSource,
        generation: Int,
        reason: RecognitionTaskCancelReason
    ) {
        emitStderr(
            "recognition-task-cancel source=\(source.rawValue) generation=\(generation) reason=\(reason.rawValue)"
        )
        task.cancel()
    }

    func stream(
        _ stream: SCStream,
        didOutputSampleBuffer sampleBuffer: CMSampleBuffer,
        of outputType: SCStreamOutputType
    ) {
        guard outputType == .audio, !isTerminal(), isSourceActive(.speaker) else { return }
        let frameCount = UInt64(CMSampleBufferGetNumSamples(sampleBuffer))
        do {
            let buffer = try pcmBuffer(from: sampleBuffer)
            processReceivedAudioBuffer(
                buffer,
                for: .speaker,
                frameCount: frameCount,
                appendTo: enqueuedAudioBufferAppendTarget(for: .speaker)
            )
        } catch {
            recordReceivedBuffer(for: .speaker, frameCount: frameCount, volume: nil)
            reportAudioConversionError(error, for: .speaker)
        }
    }

    func stream(_ stream: SCStream, didStopWithError error: Error) {
        let details = errorDetails(error)
        DispatchQueue.main.async { [weak self] in
            self?.disableSource(
                .speaker,
                kind: "screen-capture",
                message: "スピーカーの音声入力が停止しました: \(details)"
            )
        }
    }

    func cancel() {
        DispatchQueue.main.async { [weak self] in
            self?.close()
        }
    }

    private func isNoSpeechError(_ error: Error) -> Bool {
        let nsError = error as NSError
        if nsError.domain == "kAFAssistantErrorDomain", nsError.code == noSpeechDetectedErrorCode {
            return true
        }
        let normalized = [
            nsError.localizedDescription,
            nsError.localizedFailureReason ?? "",
            String(describing: error),
        ]
        .joined(separator: " ")
        .lowercased()
        return [
            "no speech detected",
            "no speech was detected",
            "speech not detected",
            "音声を検出できません",
            "音声が検出されません",
            "話し声を検出できません",
        ].contains { normalized.contains($0) }
    }

    private func errorDetails(_ error: Error) -> String {
        let nsError = error as NSError
        return "domain=\(nsError.domain) code=\(nsError.code) description=\(nsError.localizedDescription)"
    }

    private func fail(_ kind: String, _ message: String) {
        guard !isTerminal() else { return }
        emit(["event": "error", "kind": kind, "message": message])
        close()
    }

    private func disableSource(_ source: AudioSource, kind: String, message: String) {
        let timeoutWorkItem: DispatchWorkItem?
        let cancellationWorkItem: DispatchWorkItem?
        let noActiveSources: Bool
        sourceLock.lock()
        guard !terminal, sourceAvailability.disable(source) else {
            pendingSourceDisables.remove(source)
            sourceLock.unlock()
            return
        }
        pendingSourceDisables.remove(source)
        if source == .microphone {
            debugDumpOnlyRequest = nil
            debugDumpOnlyGeneration = nil
        }
        recognizers.removeValue(forKey: source)
        timeoutWorkItem = recognitionTimeoutWorkItems.removeValue(forKey: source)
        cancellationWorkItem = recognitionCancellationWorkItems.removeValue(forKey: source)
        noActiveSources = !sourceAvailability.hasActiveSource
        sourceLock.unlock()

        timeoutWorkItem?.cancel()
        cancellationWorkItem?.cancel()
        switch source {
        case .microphone:
            debugInputPlayer?.stop()
            debugInputPlayer = nil
            if let engine = audioEngine {
                if engine.isRunning { engine.stop() }
                if tapInstalled { engine.inputNode.removeTap(onBus: 0) }
            }
            audioEngine = nil
            microphoneStarted = false
            tapInstalled = false
        case .speaker:
            if let stream = speakerStream {
                speakerStream = nil
                Task { try? await stream.stopCapture() }
            }
            speakerStarted = false
        }

        let recognitionState = syncOnAudioProcessingQueue {
            let state: RecognitionState<
                SFSpeechAudioBufferRecognitionRequest,
                SFSpeechRecognitionTask,
                SFSpeechRecognizer
            >?
            if let generation = recognitionStates.currentGeneration(for: source) {
                state = recognitionStates.remove(source: source, generation: generation)
                if state == nil {
                    recognitionStates.retireGeneration(source: source, generation: generation)
                }
            } else {
                state = nil
            }
            self.pendingDrainWorkItems[source]?.cancel()
            self.pendingDrainWorkItems.removeValue(forKey: source)
            self.recognitionStates.clearPendingAndPreRoll(for: source)
            self.voiceActivity[source]?.resetToWaiting()
            return state
        }
        appendedAudioDump?.close(source: source)
        if let state = recognitionState,
           !state.taskTerminalArrived,
           !state.taskCancellationRequested {
            cancelRecognitionTask(
                state.task,
                for: state.source,
                generation: state.generation,
                reason: .sourceDisabled
            )
        }

        emit([
            "event": "error",
            "kind": kind,
            "message": "source=\(source.rawValue) \(message)",
        ])
        if noActiveSources {
            emit([
                "event": "error",
                "kind": "no-input-source",
                "message": "利用可能な音声入力源がありません",
            ])
            close()
        } else if let microphoneAuthorization,
                  let recognitionAuthorization {
            emitReadyIfPossible(microphoneAuthorization, recognitionAuthorization)
        }
    }

    private func close() {
        statsTimer?.cancel()
        statsTimer = nil
        noBufferWarningTimer?.cancel()
        noBufferWarningTimer = nil
        sourceLock.lock()
        let recognitionTimeouts = Array(recognitionTimeoutWorkItems.values)
        recognitionTimeoutWorkItems.removeAll()
        let recognitionCancellations = Array(recognitionCancellationWorkItems.values)
        recognitionCancellationWorkItems.removeAll()
        guard !terminal else {
            sourceLock.unlock()
            for workItem in recognitionTimeouts { workItem.cancel() }
            for workItem in recognitionCancellations { workItem.cancel() }
            return
        }
        terminal = true
        recognizers.removeAll()
        debugDumpOnlyRequest = nil
        debugDumpOnlyGeneration = nil
        sourceLock.unlock()
        let recognitionStatesToCancel = syncOnAudioProcessingQueue {
            for source in AudioSource.allCases {
                pendingDrainWorkItems[source]?.cancel()
                pendingDrainWorkItems.removeValue(forKey: source)
                recognitionStates.clearPendingAndPreRoll(for: source)
                voiceActivity[source]?.resetToWaiting()
            }
            return recognitionStates.removeAll()
        }
        for workItem in recognitionTimeouts { workItem.cancel() }
        for workItem in recognitionCancellations { workItem.cancel() }
        for state in recognitionStatesToCancel
            where !state.taskTerminalArrived && !state.taskCancellationRequested {
            cancelRecognitionTask(
                state.task,
                for: state.source,
                generation: state.generation,
                reason: .sessionClosed
            )
        }
        debugInputPlayer?.stop()
        debugInputPlayer = nil
        if let engine = audioEngine {
            if engine.isRunning { engine.stop() }
            if tapInstalled { engine.inputNode.removeTap(onBus: 0) }
        }
        if let stream = speakerStream {
            Task { try? await stream.stopCapture() }
        }
        appendedAudioDump?.close()
        emit(["event": "closed"])
        fflush(stdout)
        exit(0)
    }

    private func isTerminal() -> Bool {
        sourceLock.lock()
        let terminal = self.terminal
        sourceLock.unlock()
        return terminal
    }

    private func isSourceActive(_ source: AudioSource) -> Bool {
        sourceLock.lock()
        let active = !terminal && sourceAvailability.isActive(source)
        sourceLock.unlock()
        return active
    }

    private func startDiagnostics() {
        guard statsTimer == nil else { return }
        let vad = VoiceActivityConfiguration.standard
        emitStderr(
            "audio-vad noiseFloorEmaRate=\(vad.noiseFloorAdaptationRate) noiseFloorMaxRisePerSecond=\(vad.maximumNoiseFloorRiseFractionPerSecond) startMultiplier=\(vad.startNoiseMultiplier) sustainMultiplier=\(vad.sustainNoiseMultiplier) minimumStartRms=\(vad.minimumStartRmsThreshold) maximumStartRms=\(vad.maximumStartRmsThreshold) minimumSustainRms=\(vad.minimumSustainRmsThreshold) maximumSustainRms=\(vad.maximumSustainRmsThreshold) movingWindowSeconds=\(Double(vad.movingRmsWindowNanoseconds) / 1_000_000_000) startAttackSeconds=\(Double(vad.startAttackNanoseconds) / 1_000_000_000) rearmQuietSeconds=\(Double(vad.rearmQuietNanoseconds) / 1_000_000_000) preRollSeconds=\(Double(vad.preRollNanoseconds) / 1_000_000_000) steadyNoiseWindowSeconds=\(Double(vad.steadyNoiseWindowNanoseconds) / 1_000_000_000) steadyNoiseDurationSeconds=\(Double(vad.steadyNoiseDurationNanoseconds) / 1_000_000_000) steadyNoiseMaxCV=\(vad.steadyNoiseMaximumCoefficientOfVariation) steadyNoiseMaxStartRatio=\(vad.steadyNoiseMaximumStartRatio) maximumSegmentSeconds=\(Double(vad.maximumSegmentNanoseconds) / 1_000_000_000) trailingSeconds=\(Double(vad.trailingNanoseconds) / 1_000_000_000) finalTimeoutSeconds=\(Double(recognitionFinalTimeoutNanoseconds) / 1_000_000_000)"
        )
        let statsTimer = DispatchSource.makeTimerSource(
            queue: DispatchQueue(label: "dev.nrslib.coosenpai.hearing.stats", qos: .utility)
        )
        statsTimer.schedule(deadline: .now() + .seconds(30), repeating: .seconds(30))
        statsTimer.setEventHandler { [weak self] in
            self?.emitAudioStats()
        }
        self.statsTimer = statsTimer
        statsTimer.resume()

        let noBufferWarningTimer = DispatchSource.makeTimerSource(
            queue: DispatchQueue(label: "dev.nrslib.coosenpai.hearing.stats-warning", qos: .utility)
        )
        noBufferWarningTimer.schedule(deadline: .now() + .seconds(10))
        noBufferWarningTimer.setEventHandler { [weak self] in
            self?.emitNoBufferWarning()
        }
        self.noBufferWarningTimer = noBufferWarningTimer
        noBufferWarningTimer.resume()
    }

    private func recordRecognitionRestart(
        for source: AudioSource
    ) -> RecognitionRestartDecision? {
        let now = monotonicNanoseconds()
        sourceLock.lock()
        guard !terminal, sourceAvailability.isActive(source) else {
            sourceLock.unlock()
            return nil
        }
        var tracker = restartTrackers[source, default: RecognitionRestartTracker()]
        let thresholdReached = tracker.recordRestart(at: now)
        let count = tracker.recentRestartCount
        restartTrackers[source] = tracker
        sourceLock.unlock()
        return RecognitionRestartDecision(
            recentCount: count,
            thresholdReached: thresholdReached
        )
    }

    private func endRecognition(
        for source: AudioSource,
        reason: RecognitionSegmentCloseReason
    ) {
        let state: RecognitionState<
            SFSpeechAudioBufferRecognitionRequest,
            SFSpeechRecognitionTask,
            SFSpeechRecognizer
        >
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              let generation = recognitionStates.currentGeneration(for: source),
              let endingState = recognitionStates.beginEnding(
                  source: source,
                  generation: generation,
                  reason: reason
              )
        else {
            sourceLock.unlock()
            return
        }
        state = endingState
        sourceLock.unlock()

        emitRecognitionSegmentClose(
            source: source,
            generation: generation,
            reason: reason
        )
        state.request.endAudio()
        emitStderr(
            "recognition-input-ended source=\(source.rawValue) generation=\(generation)"
        )

        let workItem = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.audioProcessingQueue.async { [weak self] in
                self?.cancelRecognitionAfterTimeout(
                    for: source,
                    generation: generation
                )
            }
        }
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.lifecycle(for: source) == .ending,
              recognitionStates.isCurrentState(source, generation) else {
            sourceLock.unlock()
            workItem.cancel()
            return
        }
        let previousWorkItem = recognitionTimeoutWorkItems.updateValue(workItem, forKey: source)
        sourceLock.unlock()
        previousWorkItem?.cancel()
        DispatchQueue.main.asyncAfter(
            deadline: .now() + .nanoseconds(Int(recognitionFinalTimeoutNanoseconds)),
            execute: workItem
        )
    }

    private func cancelRecognitionAfterTimeout(
        for source: AudioSource,
        generation: Int
    ) {
        let cancellationStartedAt = monotonicNanoseconds()
        let state: RecognitionState<
            SFSpeechAudioBufferRecognitionRequest,
            SFSpeechRecognitionTask,
            SFSpeechRecognizer
        >
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              let cancellingState = recognitionStates.beginCancelling(
                  source: source,
                  generation: generation
              )
        else {
            sourceLock.unlock()
            return
        }
        state = cancellingState
        recognitionTimeoutWorkItems.removeValue(forKey: source)
        sourceLock.unlock()

        emitStderr(
            "recognition-final-timeout source=\(source.rawValue) generation=\(generation)"
        )
        appendedAudioDump?.close(source: source, generation: generation)
        cancelRecognitionTask(
            state.task,
            for: source,
            generation: generation,
            reason: .finalTimeout
        )

        let cancellationDeadline = RecognitionCancellationDeadline(
            startedAt: cancellationStartedAt,
            timeoutNanoseconds: recognitionCancellationTimeoutNanoseconds
        )
        let cancellationWorkItem = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.audioProcessingQueue.async { [weak self] in
                self?.forceCancelRecognitionAfterTimeout(
                    for: source,
                    generation: generation,
                    stage: .cancellationTimeout
                )
            }
        }
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.lifecycle(for: source) == .cancelling,
              recognitionStates.isCurrentState(source, generation) else {
            sourceLock.unlock()
            cancellationWorkItem.cancel()
            return
        }
        let previousWorkItem = recognitionCancellationWorkItems.updateValue(
            cancellationWorkItem,
            forKey: source
        )
        sourceLock.unlock()
        previousWorkItem?.cancel()
        DispatchQueue.main.asyncAfter(
            deadline: .now() + .nanoseconds(
                cancellationDeadline.delayNanoseconds(from: monotonicNanoseconds())
            ),
            execute: cancellationWorkItem
        )
    }

    private func forceCancelRecognitionAfterTimeout(
        for source: AudioSource,
        generation: Int,
        stage: RecognitionCancellationTimeoutStage
    ) {
        let taskTerminalArrived: Bool
        let debugInputHasEnded: Bool
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.lifecycle(for: source) == .cancelling,
              recognitionStates.isCurrentState(source, generation) else {
            sourceLock.unlock()
            return
        }
        taskTerminalArrived = recognitionStates.taskTerminalArrived(for: source)
        debugInputHasEnded = debugInputEnded
        if stage == .cancellationTimeout,
           !taskTerminalArrived,
           !debugInputHasEnded {
            let graceWorkItem = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.audioProcessingQueue.async { [weak self] in
                    self?.forceCancelRecognitionAfterTimeout(
                        for: source,
                        generation: generation,
                        stage: .graceExpired
                    )
                }
            }
            let previousWorkItem = recognitionCancellationWorkItems.updateValue(
                graceWorkItem,
                forKey: source
            )
            sourceLock.unlock()
            previousWorkItem?.cancel()
            emitStderr(
                "recognition-cancellation-timeout source=\(source.rawValue) generation=\(generation) terminalArrived=false action=wait-for-terminal-handler graceSeconds=\(Double(recognitionCancellationGraceNanoseconds) / 1_000_000_000)"
            )
            DispatchQueue.main.asyncAfter(
                deadline: .now() + .nanoseconds(
                    Int(recognitionCancellationGraceNanoseconds)
                ),
                execute: graceWorkItem
            )
            return
        }
        recognitionCancellationWorkItems.removeValue(forKey: source)
        sourceLock.unlock()

        let action: String
        if taskTerminalArrived {
            action = "wait-for-terminal-handler"
        } else if debugInputHasEnded {
            action = "recover-at-debug-eof"
        } else {
            action = "recover-after-grace"
        }
        emitStderr(
            "recognition-cancellation-timeout source=\(source.rawValue) generation=\(generation) terminalArrived=\(taskTerminalArrived) action=\(action)"
        )
        guard !taskTerminalArrived else { return }
        recoverRecognitionAfterCancellationTimeout(
            for: source,
            generation: generation
        )
    }

    private func recoverRecognitionAfterCancellationTimeout(
        for source: AudioSource,
        generation: Int
    ) {
        let timeoutWorkItem: DispatchWorkItem?
        let cancellationWorkItem: DispatchWorkItem?
        let recoveryCount: Int
        let shouldDisable: Bool
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.recoverCancellationTimeout(
                  source: source,
                  generation: generation
              ) != nil else {
            sourceLock.unlock()
            return
        }
        timeoutWorkItem = recognitionTimeoutWorkItems.removeValue(forKey: source)
        cancellationWorkItem = recognitionCancellationWorkItems.removeValue(
            forKey: source
        )
        var tracker = cancellationTimeoutRecoveryTrackers[
            source,
            default: RecognitionCancellationTimeoutRecoveryTracker(),
        ]
        shouldDisable = tracker.recordRecovery()
        recoveryCount = tracker.consecutiveRecoveries
        cancellationTimeoutRecoveryTrackers[source] = tracker
        sourceLock.unlock()

        timeoutWorkItem?.cancel()
        cancellationWorkItem?.cancel()
        pendingDrainWorkItems[source]?.cancel()
        pendingDrainWorkItems.removeValue(forKey: source)
        recognitionStates.markPendingReady(for: source)
        recognitionStates.clearPendingAndPreRoll(for: source)
        voiceActivity[source]?.resetToWaiting()
        appendedAudioDump?.close(source: source, generation: generation)
        emitRecognitionTaskFinished(
            source: source,
            generation: generation,
            outcome: "cancelled-timeout"
        )
        let recoveryAction = shouldDisable ? "disable-source" : "armed"
        emitStderr(
            "recognition-cancellation-recovered source=\(source.rawValue) generation=\(generation) consecutive=\(recoveryCount) action=\(recoveryAction)"
        )
        if shouldDisable {
            disableSource(
                source,
                kind: "recognition-\(source.rawValue)",
                message: "音声認識のキャンセルタイムアウトが連続しています: consecutive=\(recoveryCount)"
            )
        } else if isDebugInputEnded() {
            closeDebugInputIfFinished()
        }
    }

    private func finishCompletedRecognitionSegment(
        for source: AudioSource,
        cooldownNanoseconds: UInt64,
        rearmImmediately: Bool = false
    ) {
        let now = monotonicNanoseconds()
        voiceActivity[source]?.finishSegment(
            at: now,
            rearmImmediately: rearmImmediately
        )
        _ = recognitionStates.takePreRoll(for: source)
        let action = recognitionStates.finishPendingSegment(
            for: source,
            cooldownNanoseconds: cooldownNanoseconds,
            at: now
        )
        switch action {
        case .none:
            closeDebugInputIfFinished()
            return
        case .drainNow:
            drainPendingAudio(for: source)
        case let .drainAfter(deadline):
            schedulePendingDrain(for: source, at: deadline)
        }
    }

    private func schedulePendingDrain(for source: AudioSource, at deadline: UInt64) {
        let workItem = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.audioProcessingQueue.async { [weak self] in
                guard let self,
                      !self.isTerminal(),
                      self.isSourceActive(source),
                      !self.recognitionStates.isPendingBlocked(
                          for: source,
                          at: monotonicNanoseconds()
                      )
                else {
                    return
                }
                self.drainPendingAudio(for: source)
            }
        }
        let previousWorkItem = pendingDrainWorkItems.updateValue(workItem, forKey: source)
        previousWorkItem?.cancel()
        let now = monotonicNanoseconds()
        let delay = deadline > now ? deadline - now : 0
        let delayNanoseconds = Int(min(delay, UInt64(Int.max)))
        DispatchQueue.main.asyncAfter(
            deadline: .now() + .nanoseconds(delayNanoseconds),
            execute: workItem
        )
    }

    private func drainPendingAudio(for source: AudioSource) {
        pendingDrainWorkItems[source]?.cancel()
        pendingDrainWorkItems.removeValue(forKey: source)
        guard isSourceActive(source) else {
            _ = recognitionStates.takePending(for: source)
            return
        }
        recognitionStates.markPendingReady(for: source)
        let pending = recognitionStates.takePending(for: source)
        guard !pending.isEmpty else {
            if isDebugInputEnded() { closeDebugInputIfFinished() }
            return
        }
        for audio in pending {
            processAudioBuffer(
                audio.buffer,
                for: source,
                rms: audio.rms,
                at: audio.timestamp
            )
        }
        if isDebugInputEnded() { finishDebugInputOnProcessingQueue() }
    }

    private func debugInputDidFinish(_ result: Result<Void, Error>) {
        audioProcessingQueue.async { [weak self] in
            guard let self else { return }
            switch result {
            case .success:
                self.sourceLock.lock()
                guard !self.terminal, self.sourceAvailability.isActive(.microphone) else {
                    self.sourceLock.unlock()
                    return
                }
                self.debugInputEnded = true
                self.sourceLock.unlock()
                self.finishDebugInputOnProcessingQueue()
            case let .failure(error):
                self.disableSource(
                    .microphone,
                    kind: "debug-input",
                    message: "デバッグ用 WAV の再生に失敗しました: \(self.errorDetails(error))"
                )
            }
        }
    }

    private func finishDebugInputOnProcessingQueue() {
        guard isSourceActive(.microphone) else { return }
        sourceLock.lock()
        let hasDebugDumpOnlyRequest = debugDumpOnlyRequest != nil
        sourceLock.unlock()
        if hasDebugDumpOnlyRequest {
            finishDebugDumpOnlyInputOnProcessingQueue()
            return
        }

        let recognitionGeneration: Int?
        let recognitionLifecycle: RecognitionSegmentLifecycle?
        let recognitionTaskTerminalArrived: Bool
        let debugInputHasEnded: Bool
        sourceLock.lock()
        recognitionGeneration = recognitionStates.currentGeneration(for: .microphone)
        recognitionLifecycle = recognitionStates.lifecycle(for: .microphone)
        recognitionTaskTerminalArrived = recognitionStates.taskTerminalArrived(
            for: .microphone
        )
        debugInputHasEnded = self.debugInputEnded
        sourceLock.unlock()
        let hasRecognition = recognitionGeneration != nil
        if debugInputHasEnded,
           let generation = recognitionGeneration,
           recognitionLifecycle == .cancelling,
           !recognitionTaskTerminalArrived {
            recoverRecognitionAfterCancellationTimeout(
                for: .microphone,
                generation: generation
            )
            closeDebugInputIfFinished()
            return
        }

        var detector = voiceActivity[
            .microphone,
            default: VoiceActivityDetector(configuration: .standard)
        ]
        let hasPendingAudio = recognitionStates.hasPendingAudio(for: .microphone)
        let hasPendingCooldown = recognitionStates.hasPendingCooldown(for: .microphone)
        if !hasRecognition && hasPendingAudio {
            switch detector.phase {
            case .finishing, .pending:
                detector.finishSegment(at: monotonicNanoseconds())
            case .waiting, .speaking, .rearming:
                break
            }
        }
        if !hasRecognition && hasPendingAudio && hasPendingCooldown {
            voiceActivity[.microphone] = detector
            recordVoiceActivity(for: .microphone, levels: detector.levels)
            closeDebugInputIfFinished()
            return
        }
        let forcedFinish = detector.forceFinish()
        if forcedFinish, !hasRecognition {
            detector.finishSegment(at: monotonicNanoseconds())
        }
        voiceActivity[.microphone] = detector
        recordVoiceActivity(for: .microphone, levels: detector.levels)
        if hasRecognition {
            endRecognition(for: .microphone, reason: .forced)
        } else if hasPendingAudio {
            drainPendingAudio(for: .microphone)
        }
        closeDebugInputIfFinished()
    }

    private func finishDebugDumpOnlyInputOnProcessingQueue() {
        let generation: Int?
        sourceLock.lock()
        generation = debugDumpOnlyGeneration
        debugDumpOnlyRequest = nil
        debugDumpOnlyGeneration = nil
        sourceLock.unlock()
        guard let generation else {
            closeDebugInputIfFinished()
            return
        }
        appendedAudioDump?.close(source: .microphone, generation: generation)
        emitStderr(
            "audio-dump-segment-close source=microphone generation=\(generation) reason=eof"
        )
        closeDebugInputIfFinished()
    }

    private func closeDebugInputIfFinished() {
        let canClose: Bool
        sourceLock.lock()
        canClose = !terminal
            && debugInputEnded
            && sourceAvailability.isActive(.microphone)
            && recognitionStates.currentGeneration(for: .microphone) == nil
            && debugDumpOnlyRequest == nil
        sourceLock.unlock()
        guard canClose else { return }
        guard !recognitionStates.hasPendingAudio(for: .microphone),
              !recognitionStates.hasPendingCooldown(for: .microphone),
              isVoiceActivityArmedOrRearming(voiceActivity[.microphone]?.phase) else {
            return
        }
        close()
    }

    private func isVoiceActivityArmedOrRearming(
        _ phase: VoiceActivityPhase?
    ) -> Bool {
        guard let phase else { return true }
        switch phase {
        case .waiting, .rearming:
            return true
        case .speaking, .finishing, .pending:
            return false
        }
    }

    private func isDebugInputEnded() -> Bool {
        sourceLock.lock()
        let ended = debugInputEnded
        sourceLock.unlock()
        return ended
    }

    private func bufferPendingAudio(
        _ buffer: AVAudioPCMBuffer,
        for source: AudioSource,
        rms: Double,
        at timestamp: UInt64
    ) {
        guard let durationNanoseconds = audioDurationNanoseconds(for: buffer) else {
            disableSource(
                source,
                kind: "audio-format",
                message: "保留対象の音声バッファの時間を計算できません: \(audioFormatDescription(buffer.format))"
            )
            return
        }
        let result = recognitionStates.appendPending(
            PendingAudioBuffer(buffer: buffer, rms: rms, timestamp: timestamp),
            for: source,
            durationNanoseconds: durationNanoseconds
        )
        if case let .droppedOldest(count) = result {
            sourceLock.lock()
            let shouldReport = reportedPendingOverflows.insert(source).inserted
            sourceLock.unlock()
            if shouldReport {
                emitStderr(
                    "audio-stats-warning pending-overflow source=\(source.rawValue) droppedBuffers=\(count) capacitySeconds=\(Double(pendingAudioWindowCapacityNanoseconds) / 1_000_000_000)"
                )
            }
        } else if result == .rejected {
            emit([
                "event": "warning",
                "kind": "audio-pending",
                "message": "source=\(source.rawValue) 認識終了待ちの音声バッファを保持できませんでした: durationSeconds=\(Double(durationNanoseconds) / 1_000_000_000)",
            ])
            emitStderr(
                "audio-stats-warning pending-rejected source=\(source.rawValue) durationSeconds=\(Double(durationNanoseconds) / 1_000_000_000)"
            )
        }
    }

    private func reportAudioConversionError(_ error: Error, for source: AudioSource) {
        sourceLock.lock()
        let shouldReport = reportedAudioConversionErrors.insert(source).inserted
        sourceLock.unlock()
        let details = errorDetails(error)
        recordAudioInputFailure(
            for: source,
            kind: "audio-conversion",
            message: "\(source.rawValue) の CMSampleBuffer を PCM に変換できませんでした: \(details)",
            shouldReport: shouldReport
        )
    }

    private func reportAudioMonoConversionError(_ error: Error, for source: AudioSource) {
        sourceLock.lock()
        let shouldReport = reportedAudioMonoConversionErrors.insert(source).inserted
        sourceLock.unlock()
        let details = errorDetails(error)
        recordAudioInputFailure(
            for: source,
            kind: "audio-conversion",
            message: "\(source.rawValue) の音声バッファを mono float32 に変換できませんでした: \(details)",
            shouldReport: shouldReport
        )
    }

    private func reportAudioBufferCopyError(_ error: Error, for source: AudioSource) {
        sourceLock.lock()
        let shouldReport = reportedAudioBufferCopyErrors.insert(source).inserted
        sourceLock.unlock()
        let details = errorDetails(error)
        recordAudioInputFailure(
            for: source,
            kind: "audio-buffer-copy",
            message: "\(source.rawValue) の音声バッファを複製できませんでした: \(details)",
            shouldReport: shouldReport
        )
    }

    private func reportAudioVolumeError(for source: AudioSource, format: AVAudioFormat) {
        sourceLock.lock()
        let shouldReport = reportedAudioVolumeErrors.insert(source).inserted
        sourceLock.unlock()
        let description = audioFormatDescription(format)
        recordAudioInputFailure(
            for: source,
            kind: "audio-format",
            message: "\(source.rawValue) の音声バッファから音量を計算できません: \(description)",
            shouldReport: shouldReport
        )
    }

    private func reportAudioScaleWarning(
        for source: AudioSource,
        peak: Double,
        format: AVAudioFormat,
        buffer: AVAudioPCMBuffer,
        summary: AudioSampleClampSummary
    ) {
        sourceLock.lock()
        let shouldReport = reportedAudioScaleWarnings.insert(source).inserted
        sourceLock.unlock()
        guard shouldReport else { return }

        var reasons: [String] = []
        if summary.nonFiniteCount > 0 { reasons.append("非有限サンプル") }
        if summary.outOfRangeCount > 0 || peak > 1 { reasons.append("範囲外サンプル") }
        let reason = reasons.joined(separator: "・")
        emitStderr(
            "audio-stats-warning audio-scale source=\(source.rawValue) peak=\(String(format: "%.4f", peak)) sampleCount=\(summary.sampleCount) clipped=\(summary.clippedSampleCount) clipRatio=\(String(format: "%.6f", summary.clippedSampleRatio)) outOfRange=\(summary.outOfRangeCount) nonFinite=\(summary.nonFiniteCount) \(reason) \(audioBufferLayoutDescription(buffer, format: format)); append前に[-1,1]へclamp"
        )
    }

    private func recordAudioInputFailure(
        for source: AudioSource,
        kind: String,
        message: String,
        shouldReport: Bool
    ) {
        let failureCount: Int
        let shouldDisable: Bool
        sourceLock.lock()
        guard !terminal, sourceAvailability.isActive(source) else {
            sourceLock.unlock()
            return
        }
        var tracker = audioInputFailureTrackers[
            source,
            default: AudioInputFailureTracker(),
        ]
        let thresholdReached = tracker.recordFailure()
        failureCount = tracker.consecutiveFailures
        audioInputFailureTrackers[source] = tracker
        shouldDisable = thresholdReached && !pendingSourceDisables.contains(source)
        if shouldDisable {
            pendingSourceDisables.insert(source)
        }
        sourceLock.unlock()

        if shouldReport {
            emit(["event": "error", "kind": kind, "message": message])
        }
        guard shouldDisable else { return }
        let disableMessage = "\(message) 連続失敗数=\(failureCount) 閾値=\(AudioInputFailureTracker.maximumConsecutiveFailures)"
        DispatchQueue.main.async { [weak self] in
            self?.disableSource(
                source,
                kind: "audio-input-failure",
                message: disableMessage
            )
        }
    }

    private func recordAudioInputSuccess(for source: AudioSource) {
        sourceLock.lock()
        guard !terminal, sourceAvailability.isActive(source) else {
            sourceLock.unlock()
            return
        }
        var tracker = audioInputFailureTrackers[
            source,
            default: AudioInputFailureTracker(),
        ]
        tracker.recordSuccessfulBuffer()
        audioInputFailureTrackers[source] = tracker
        sourceLock.unlock()
    }

    private func enqueuedAudioBufferAppendTarget(
        for source: AudioSource
    ) -> EnqueuedAudioBufferAppendTarget {
        EnqueuedAudioBufferAppendTarget { [weak self] buffer, rms in
            self?.enqueue(buffer, for: source, rms: rms)
        }
    }

    private func receive(_ buffer: AVAudioPCMBuffer, for source: AudioSource) {
        guard !isTerminal(), isSourceActive(source) else { return }
        processReceivedAudioBuffer(
            buffer,
            for: source,
            frameCount: UInt64(buffer.frameLength),
            appendTo: enqueuedAudioBufferAppendTarget(for: source)
        )
    }

    func processReceivedAudioBuffer(
        _ buffer: AVAudioPCMBuffer,
        for source: AudioSource,
        frameCount: UInt64,
        appendTo target: AudioBufferAppendTarget
    ) {
        guard !isTerminal(), isSourceActive(source) else { return }
        let processed: ReceivedAudioBufferProcessingResult
        do {
            processed = try ReceivedAudioBufferProcessor.processReceivedAudioBuffer(
                buffer,
                appendTo: target
            )
        } catch let error as ReceivedAudioBufferProcessingError {
            switch error {
            case let .copy(error):
                recordReceivedBuffer(for: source, frameCount: frameCount, volume: nil)
                reportAudioBufferCopyError(error, for: source)
            case let .normalization(error):
                recordReceivedBuffer(for: source, frameCount: frameCount, volume: nil)
                reportAudioMonoConversionError(error, for: source)
            case let .volumeUnavailable(format):
                recordReceivedBuffer(for: source, frameCount: frameCount, volume: nil)
                reportAudioVolumeError(for: source, format: format)
            }
            return
        } catch {
            recordReceivedBuffer(for: source, frameCount: frameCount, volume: nil)
            reportAudioVolumeError(for: source, format: buffer.format)
            return
        }
        reportReceivedFormat(processed.receivedFormat, for: source)
        if processed.rawVolume.peak > 1 || processed.clampSummary.hasAnomaly {
            reportAudioScaleWarning(
                for: source,
                peak: processed.rawVolume.peak,
                format: processed.buffer.format,
                buffer: processed.buffer,
                summary: processed.clampSummary
            )
        }
        recordAudioInputSuccess(for: source)
        recordReceivedBuffer(
            for: source,
            frameCount: frameCount,
            volume: processed.rawVolume
        )
        guard processed.clampedVolume != nil else {
            reportAudioVolumeError(for: source, format: processed.buffer.format)
            return
        }
    }

    private func enqueue(_ buffer: AVAudioPCMBuffer, for source: AudioSource, rms: Double) {
        let timestamp = monotonicNanoseconds()
        audioProcessingQueue.async { [weak self] in
            self?.processAudioBuffer(buffer, for: source, rms: rms, at: timestamp)
        }
    }

    private func reportReceivedFormat(_ format: AVAudioFormat, for source: AudioSource) {
        sourceLock.lock()
        let shouldReport = reportedReceivedFormats.insert(source).inserted
        sourceLock.unlock()
        guard shouldReport else { return }
        emitStderr("audio-format \(source.rawValue) received=\(audioFormatDescription(format))")
    }

    private func processAudioBuffer(
        _ buffer: AVAudioPCMBuffer,
        for source: AudioSource,
        rms: Double,
        at timestamp: UInt64
    ) {
        guard !isTerminal(), isSourceActive(source) else { return }
        guard let durationNanoseconds = audioDurationNanoseconds(for: buffer) else {
            disableSource(
                source,
                kind: "audio-format",
                message: "音声バッファの時間を計算できません: \(audioFormatDescription(buffer.format))"
            )
            return
        }
        if isDebugDumpOnly(source) {
            appendDebugDumpOnly(buffer, for: source)
            return
        }

        if let lifecycle = currentRecognitionLifecycle(for: source) {
            switch lifecycle {
            case .accepting:
                break
            case .ending, .terminal, .cancelling:
                bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
                return
            }
        }

        if recognitionStates.hasPendingCooldown(for: source) {
            if recognitionStates.isPendingBlocked(for: source, at: timestamp) {
                bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
                return
            }
            recognitionStates.markPendingReady(for: source)
            drainPendingAudio(for: source)
            guard !isTerminal(), isSourceActive(source) else { return }
            processAudioBuffer(buffer, for: source, rms: rms, at: timestamp)
            return
        }

        let action = observeVoiceActivity(
            rms: rms,
            durationNanoseconds: durationNanoseconds,
            at: timestamp,
            for: source
        )
        switch action {
        case .wait:
            recordPreRollAudio(buffer, for: source, rms: rms, at: timestamp)
            return
        case .start:
            let preRoll = takePreRollAudio(for: source)
            guard startRecognition(for: source) else {
                guard isSourceActive(source) else { return }
                voiceActivity[source]?.deferCurrentSegment()
                for audio in preRoll {
                    bufferPendingAudio(
                        audio.buffer,
                        for: source,
                        rms: audio.rms,
                        at: audio.timestamp
                    )
                }
                bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
                return
            }
            appendSegmentBuffers(
                preRoll,
                current: PendingAudioBuffer(
                    buffer: buffer,
                    rms: rms,
                    timestamp: timestamp
                ),
                for: source
            )
        case .append:
            guard append(buffer, for: source) else {
                if isSourceActive(source) {
                    bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
                }
                return
            }
        case let .appendAndFinish(reason):
            guard append(buffer, for: source) else {
                if isSourceActive(source) {
                    bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
                }
                return
            }
            let closeReason: RecognitionSegmentCloseReason
            switch reason {
            case .trailing:
                closeReason = .trailing
            case .maximum:
                closeReason = .max
            case .steadyNoise:
                closeReason = .steadyNoise
            }
            endRecognition(for: source, reason: closeReason)
        case .bufferForNextSegment:
            bufferPendingAudio(buffer, for: source, rms: rms, at: timestamp)
        }
    }

    private func currentRecognitionLifecycle(
        for source: AudioSource
    ) -> RecognitionSegmentLifecycle? {
        sourceLock.lock()
        let lifecycle = recognitionStates.lifecycle(for: source)
        sourceLock.unlock()
        return lifecycle
    }

    private func recordPreRollAudio(
        _ buffer: AVAudioPCMBuffer,
        for source: AudioSource,
        rms: Double,
        at timestamp: UInt64
    ) {
        guard let durationNanoseconds = audioDurationNanoseconds(for: buffer) else {
            disableSource(
                source,
                kind: "audio-format",
                message: "プリロール対象の音声バッファの時間を計算できません: \(audioFormatDescription(buffer.format))"
            )
            return
        }
        recognitionStates.appendPreRoll(
            PendingAudioBuffer(buffer: buffer, rms: rms, timestamp: timestamp),
            for: source,
            durationNanoseconds: durationNanoseconds
        )
    }

    private func takePreRollAudio(for source: AudioSource) -> [PendingAudioBuffer] {
        recognitionStates.takePreRoll(for: source)
    }

    private func appendSegmentBuffers(
        _ preRoll: [PendingAudioBuffer],
        current: PendingAudioBuffer,
        for source: AudioSource
    ) {
        let buffers = preRoll + [current]
        for index in buffers.indices {
            let audio = buffers[index]
            guard append(audio.buffer, for: source) else {
                guard isSourceActive(source) else { return }
                for pendingAudio in buffers[index...] {
                    bufferPendingAudio(
                        pendingAudio.buffer,
                        for: source,
                        rms: pendingAudio.rms,
                        at: pendingAudio.timestamp
                    )
                }
                return
            }
        }
    }

    private func isDebugDumpOnly(_ source: AudioSource) -> Bool {
        sourceLock.lock()
        let active = source == .microphone
            && debugDumpOnlyRequest != nil
            && debugDumpOnlyGeneration != nil
        sourceLock.unlock()
        return active
    }

    private func appendDebugDumpOnly(_ buffer: AVAudioPCMBuffer, for source: AudioSource) {
        let formatDescription = audioFormatDescription(buffer.format)
        guard let appendedAudioDump else {
            disableSource(
                source,
                kind: "debug-dump-source",
                message: "追加音声ダンプが利用できません"
            )
            return
        }
        let generation: Int
        let shouldReportFormat: Bool
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              let request = debugDumpOnlyRequest,
              let debugDumpOnlyGeneration else {
            sourceLock.unlock()
            return
        }
        generation = debugDumpOnlyGeneration
        shouldReportFormat = reportedAppendFormats.insert(source).inserted
        request.append(buffer)
        switch source {
        case .microphone:
            microphoneStats.recordAppend()
        case .speaker:
            speakerStats.recordAppend()
        }
        sourceLock.unlock()

        do {
            try appendedAudioDump.append(
                buffer,
                source: source,
                generation: generation
            )
        } catch {
            disableSource(
                source,
                kind: "debug-dump-source",
                message: "追加音声ダンプに失敗しました: \(errorDetails(error))"
            )
            return
        }
        if shouldReportFormat {
            emitStderr("audio-format \(source.rawValue) append=\(formatDescription)")
        }
    }

    private func observeVoiceActivity(
        rms: Double,
        durationNanoseconds: UInt64,
        at timestamp: UInt64,
        for source: AudioSource
    ) -> VoiceActivityAction {
        var detector = voiceActivity[
            source,
            default: VoiceActivityDetector(configuration: .standard)
        ]
        let action = detector.observe(
            rms: rms,
            durationNanoseconds: durationNanoseconds,
            at: timestamp
        )
        voiceActivity[source] = detector
        recordVoiceActivity(for: source, levels: detector.levels)
        return action
    }

    private func recordReceivedBuffer(
        for source: AudioSource,
        frameCount: UInt64,
        volume: AudioVolumeMeasurement?
    ) {
        sourceLock.lock()
        defer { sourceLock.unlock() }
        guard !terminal else { return }
        switch source {
        case .microphone:
            microphoneStats.recordBuffer(frameCount: frameCount)
            if let volume { microphoneStats.recordVolume(volume) }
        case .speaker:
            speakerStats.recordBuffer(frameCount: frameCount)
            if let volume { speakerStats.recordVolume(volume) }
        }
    }

    private func recordNoSpeechRestart(for source: AudioSource) {
        sourceLock.lock()
        defer { sourceLock.unlock() }
        guard !terminal, sourceAvailability.isActive(source) else { return }
        switch source {
        case .microphone:
            microphoneStats.recordNoSpeechRestart()
        case .speaker:
            speakerStats.recordNoSpeechRestart()
        }
    }

    private func recordVoiceActivity(for source: AudioSource, levels: VoiceActivityLevels) {
        sourceLock.lock()
        defer { sourceLock.unlock() }
        guard !terminal, sourceAvailability.isActive(source) else { return }
        switch source {
        case .microphone:
            microphoneStats.recordVoiceActivity(
                noiseFloorRms: levels.noiseFloorRms,
                startRmsThreshold: levels.startRmsThreshold,
                sustainRmsThreshold: levels.sustainRmsThreshold
            )
        case .speaker:
            speakerStats.recordVoiceActivity(
                noiseFloorRms: levels.noiseFloorRms,
                startRmsThreshold: levels.startRmsThreshold,
                sustainRmsThreshold: levels.sustainRmsThreshold
            )
        }
    }

    @discardableResult
    private func append(_ buffer: AVAudioPCMBuffer, for source: AudioSource) -> Bool {
        let formatDescription = audioFormatDescription(buffer.format)
        let appendedAudioDump: AppendedAudioDump?
        if debugDumpAppendedPath != nil {
            guard let dump = self.appendedAudioDump else {
                disableSource(
                    source,
                    kind: "debug-dump-source",
                    message: "追加音声ダンプが利用できません"
                )
                return false
            }
            appendedAudioDump = dump
        } else {
            appendedAudioDump = nil
        }
        let generation: Int
        sourceLock.lock()
        guard !terminal,
              sourceAvailability.isActive(source),
              recognitionStates.acceptsAudio(for: source),
              let request = recognitionStates.currentRequest(for: source),
              let currentGeneration = recognitionStates.currentGeneration(for: source)
        else {
            sourceLock.unlock()
            return false
        }
        generation = currentGeneration
        let shouldReportFormat = reportedAppendFormats.insert(source).inserted
        request.append(buffer)
        switch source {
        case .microphone:
            microphoneStats.recordAppend()
        case .speaker:
            speakerStats.recordAppend()
        }
        sourceLock.unlock()
        do {
            try appendedAudioDump?.append(buffer, source: source, generation: generation)
        } catch {
            disableSource(
                source,
                kind: "debug-dump-source",
                message: "追加音声ダンプに失敗しました: \(errorDetails(error))"
            )
            return false
        }
        if shouldReportFormat {
            emitStderr("audio-format \(source.rawValue) append=\(formatDescription)")
        }
        return true
    }

    private func audioStatsSnapshot(resetVolume: Bool) -> AudioStatsSnapshot {
        sourceLock.lock()
        let snapshot = AudioStatsSnapshot(
            microphone: microphoneStats,
            speaker: speakerStats
        )
        if resetVolume {
            microphoneStats.resetVolumeWindow()
            speakerStats.resetVolumeWindow()
        }
        sourceLock.unlock()
        return snapshot
    }

    private func emitAudioStats() {
        guard !isTerminal() else { return }
        let snapshot = audioStatsSnapshot(resetVolume: true)
        emitStderr(
            "audio-stats \(audioStatsLine(sources: sources, snapshot: snapshot))"
        )
    }

    private func emitNoBufferWarning() {
        guard !isTerminal() else { return }
        let snapshot = audioStatsSnapshot(resetVolume: false)
        let missingSources = missingAudioSources(sources: sources, snapshot: snapshot)
        guard !missingSources.isEmpty else { return }
        let missing = missingSources.map(\.rawValue).joined(separator: ",")
        emitStderr(
            "audio-stats-warning no-buffers-after-10s missing=\(missing) \(audioStatsLine(sources: sources, snapshot: snapshot))"
        )
    }
}

private enum HearingError: LocalizedError {
    case noDisplay
    case audioTap

    var errorDescription: String? {
        switch self {
        case .noDisplay: return "音声を取得できるディスプレイが見つかりません"
        case .audioTap: return "マイクの音声タップを設置できません"
        }
    }
}

struct Arguments {
    let locale: Locale
    let inputDevice: String
    let sources: Set<AudioSource>
    let debugInputWavPath: String?
    let debugDumpAppendedPath: String?
    let debugRequestAuth: Bool
}

func parseArguments() -> Arguments {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.count >= 6,
          arguments[0] == "--locale",
          arguments[2] == "--input-device",
          arguments[4] == "--sources" else {
        emit(["event": "error", "kind": "arguments", "message": "--locale、--input-device、--sources を指定してください。任意で --debug-input-wav <path>、--debug-dump-appended <dir>、または --debug-request-auth を追加できます"])
        exit(2)
    }
    var debugInputWavPath: String?
    var debugDumpAppendedPath: String?
    var debugRequestAuth = false
    var index = 6
    while index < arguments.count {
        switch arguments[index] {
        case "--debug-input-wav":
            guard debugInputWavPath == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emit(["event": "error", "kind": "arguments", "message": "デバッグ入力は --debug-input-wav <path> で指定してください"])
                exit(2)
            }
            debugInputWavPath = arguments[index + 1]
            index += 2
        case "--debug-dump-appended":
            guard debugDumpAppendedPath == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emit(["event": "error", "kind": "arguments", "message": "追加音声ダンプは --debug-dump-appended <dir> で指定してください"])
                exit(2)
            }
            debugDumpAppendedPath = arguments[index + 1]
            index += 2
        case "--debug-request-auth":
            guard !debugRequestAuth else {
                emit(["event": "error", "kind": "arguments", "message": "--debug-request-auth は一度だけ指定してください"])
                exit(2)
            }
            debugRequestAuth = true
            index += 1
        default:
            emit(["event": "error", "kind": "arguments", "message": "未対応の引数です: \(arguments[index])"])
            exit(2)
        }
    }
    let sourceValues = arguments[5].split(separator: ",").compactMap { AudioSource(rawValue: String($0)) }
    let sources = Set(sourceValues)
    guard !sources.isEmpty, sources.count == sourceValues.count else {
        emit(["event": "error", "kind": "arguments", "message": "--sources は microphone または speaker で指定してください"])
        exit(2)
    }
    guard debugInputWavPath == nil || sources.contains(.microphone) else {
        emit(["event": "error", "kind": "arguments", "message": "--debug-input-wav は microphone source と一緒に指定してください"])
        exit(2)
    }
    let locale = arguments[1] == "system" ? Locale.current : Locale(identifier: arguments[1])
    return Arguments(
        locale: locale,
        inputDevice: arguments[3],
        sources: sources,
        debugInputWavPath: debugInputWavPath,
        debugDumpAppendedPath: debugDumpAppendedPath,
        debugRequestAuth: debugRequestAuth
    )
}

private enum InputDeviceError: LocalizedError {
    case unavailable
    case audioUnit

    var errorDescription: String? {
        switch self {
        case .unavailable: return "選択したマイクが見つかりません"
        case .audioUnit: return "選択したマイクを音声入力へ設定できません"
        }
    }
}

private func selectInputDevice(_ uniqueID: String, inputNode: AVAudioInputNode) throws {
    guard AVCaptureDevice(uniqueID: uniqueID) != nil,
          let deviceID = coreAudioDeviceID(uniqueID: uniqueID) else {
        throw InputDeviceError.unavailable
    }
    try setInputDevice(deviceID, inputNode: inputNode)
}

private func useDefaultInputDevice(inputNode: AVAudioInputNode) throws {
    guard let deviceID = defaultInputDeviceID() else { throw InputDeviceError.unavailable }
    try setInputDevice(deviceID, inputNode: inputNode)
}

private func setInputDevice(_ deviceID: AudioDeviceID, inputNode: AVAudioInputNode) throws {
    guard let audioUnit = inputNode.audioUnit else { throw InputDeviceError.audioUnit }
    var selected = deviceID
    let status = AudioUnitSetProperty(
        audioUnit,
        kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global,
        0,
        &selected,
        UInt32(MemoryLayout<AudioDeviceID>.size)
    )
    guard status == noErr else { throw InputDeviceError.audioUnit }
}

private func defaultInputDeviceID() -> AudioDeviceID? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultInputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var deviceID = AudioDeviceID(0)
    var size = UInt32(MemoryLayout<AudioDeviceID>.size)
    guard AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &deviceID
    ) == noErr else { return nil }
    return deviceID
}

private func coreAudioDeviceID(uniqueID: String) -> AudioDeviceID? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(
        AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size
    ) == noErr else { return nil }
    let count = Int(size) / MemoryLayout<AudioDeviceID>.size
    var devices = [AudioDeviceID](repeating: 0, count: count)
    guard AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &devices
    ) == noErr else { return nil }
    for device in devices {
        var uidAddress = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyDeviceUID,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var uid: Unmanaged<CFString>?
        var uidSize = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        if AudioObjectGetPropertyData(device, &uidAddress, 0, nil, &uidSize, &uid) == noErr,
           uid?.takeUnretainedValue() as String? == uniqueID {
            return device
        }
    }
    return nil
}
