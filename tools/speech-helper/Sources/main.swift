import AVFoundation
import AudioToolbox
import CoreAudio
import Foundation

private let outputLock = NSLock()

private func emit(_ value: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: value) else { return }
    outputLock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    outputLock.unlock()
}

private func diagnostic(_ message: String) {
    outputLock.lock()
    defer { outputLock.unlock() }
    FileHandle.standardError.write(Data("speech \(message)\n".utf8))
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

private final class SpeechSession: @unchecked Sendable {
    private let locale: Locale
    private let engine: SpeechEngine
    private let inputDevice: String
    private let debugInputWavPath: String?
    private let debugDumpWavPath: String?
    private let audioEngine = AVAudioEngine()
    private let audio = SpeechAudioQueue()
    private var debugInputPlayer: DebugInputWavPlayer?
    private var debugWavDump: SpeechWavDump?
    private var recognitionSession: SpeechRecognitionSession?
    private var terminal = false
    private var terminalResultEmitted = false
    private var tapInstalled = false
    private var finishRequested = false
    private var earlyFinishDeadline: DispatchWorkItem?

    init(locale: Locale, engine: SpeechEngine, inputDevice: String, debugInputWavPath: String?, debugDumpWavPath: String?) {
        self.locale = locale
        self.engine = engine
        self.inputDevice = inputDevice
        self.debugInputWavPath = debugInputWavPath
        self.debugDumpWavPath = debugDumpWavPath
    }

    func authorizeAndStart() {
        diagnostic("event=engine-selected engine=\(engine.name)")
        if debugInputWavPath != nil {
            prepareInput(microphone: AVCaptureDevice.authorizationStatus(for: .audio))
            return
        }
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        if status == .notDetermined {
            AVCaptureDevice.requestAccess(for: .audio) { [weak self] _ in
                DispatchQueue.main.async {
                    self?.handleMicrophoneAuthorization(AVCaptureDevice.authorizationStatus(for: .audio))
                }
            }
        } else {
            handleMicrophoneAuthorization(status)
        }
    }

    private func handleMicrophoneAuthorization(_ microphone: AVAuthorizationStatus) {
        guard !terminal else { return }
        guard microphone == .authorized else {
            fail("permission-microphone", "マイクの使用が許可されていません")
            return
        }
        prepareInput(microphone: microphone)
    }

    private func prepareInput(microphone: AVAuthorizationStatus) {
        guard !terminal else { return }
        do {
            let format: AVAudioFormat
            if let path = debugInputWavPath {
                let player = try DebugInputWavPlayer(path: path, playbackRate: 1)
                debugInputPlayer = player
                format = player.format
            } else {
                let input = audioEngine.inputNode
                try configureInputDevice(input)
                format = input.outputFormat(forBus: 0)
            }
            let analysis = engine.makeAnalysis(
                locale: locale, inputFormat: format,
                traceResults: debugInputWavPath != nil, diagnostic: diagnostic
            )
            let recognition = SpeechRecognitionSession(
                locale: locale, audio: audio, analysis: analysis,
                scheduler: MainQueueSpeechDeadlineScheduler(),
                startRecording: { [weak self] in self?.startRecording(format: format, microphone: microphone) },
                stopRecording: { [weak self] in self?.stopRecording() },
                emit: { [weak self] output in self?.handleOutput(output) },
                diagnostic: diagnostic
            )
            recognitionSession = recognition
            recognition.start(finishRequested: finishRequested)
        } catch let failure as SpeechAnalysisFailure {
            fail(failure.kind, failure.message)
        } catch {
            fail("audio", "音声入力を準備できませんでした")
        }
    }

    private func configureInputDevice(_ input: AVAudioInputNode) throws {
        guard inputDevice != "default" else { return }
        do {
            try selectInputDevice(inputDevice, inputNode: input)
        } catch {
            do { try useDefaultInputDevice(inputNode: input) }
            catch { throw SpeechAnalysisFailure(kind: "input-device", message: "マイクの入力デバイスを選択できませんでした") }
            emit([
                "event": "warning", "kind": "input-device-fallback",
                "message": "選択したマイクを利用できないため、システム既定を使います",
            ])
        }
    }

    private func startRecording(format: AVAudioFormat, microphone: AVAuthorizationStatus) {
        guard !terminal else { return }
        if let player = debugInputPlayer {
            startDebugInput(player, microphone: microphone)
            return
        }
        do {
            if let path = debugDumpWavPath {
                do {
                    debugWavDump = try SpeechWavDump(path: path, format: format)
                    diagnostic("event=debug-dump-start sampleRate=\(format.sampleRate) channels=\(format.channelCount)")
                } catch {
                    fail("debug-dump", error.localizedDescription)
                    return
                }
            }
            let dump = debugWavDump
            audioEngine.inputNode.installTap(onBus: 0, bufferSize: 1_024, format: format) { [weak self] buffer, _ in
                guard let self else { return }
                do {
                    try dump?.append(buffer)
                } catch {
                    let message = error.localizedDescription
                    DispatchQueue.main.async { self.fail("debug-dump", message) }
                    return
                }
                if self.audio.enqueue(buffer) {
                    DispatchQueue.main.async { self.recognitionSession?.audioAvailable() }
                }
            }
            tapInstalled = true
            audioEngine.prepare()
            try audioEngine.start()
            emit([
                "event": "ready", "locale": locale.identifier,
                "engine": engine.name,
                "microphone": permissionName(microphone), "recognition": "granted",
            ])
        } catch {
            fail("audio", "マイクの録音を開始できませんでした")
        }
    }

    private func startDebugInput(_ player: DebugInputWavPlayer, microphone: AVAuthorizationStatus) {
        diagnostic("event=debug-input-start frames=\(player.frameLength) sampleRate=\(player.format.sampleRate)")
        emit([
            "event": "ready", "locale": locale.identifier,
            "engine": engine.name,
            "microphone": permissionName(microphone), "recognition": "granted", "input": "wav",
        ])
        player.start(
            onBuffer: { [weak self] buffer in
                guard let self else { return }
                if self.audio.enqueue(buffer) {
                    DispatchQueue.main.async { self.recognitionSession?.audioAvailable() }
                }
            },
            onCompletion: { [weak self] result in
                DispatchQueue.main.async {
                    guard let self, !self.terminal else { return }
                    switch result {
                    case .success:
                        // EOF と利用者の finish を分け、テストも stdin の通常経路を通す。
                        emit(["event": "debug-input-ended"])
                    case .failure:
                        self.fail("audio", "デバッグ用 WAV の読み込みに失敗しました")
                    }
                }
            }
        )
    }

    func finish() {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.terminal, !self.finishRequested else { return }
            self.finishRequested = true
            if let recognition = self.recognitionSession {
                recognition.finish()
            } else {
                // マイク権限待ちの finish も、認識器の準備開始で30秒期限を延長しない。
                let deadline = DispatchWorkItem { [weak self] in
                    guard let self, !self.terminal else { return }
                    self.fail("recognition", "音声認識の確定処理が30秒以内に完了しませんでした")
                    self.close()
                }
                self.earlyFinishDeadline = deadline
                DispatchQueue.main.asyncAfter(deadline: .now() + SpeechRecognitionSession.finalizationTimeout, execute: deadline)
            }
        }
    }

    private func stopRecording() {
        if let debugInputPlayer {
            debugInputPlayer.stop()
            return
        }
        audioEngine.stop()
        removeTap()
    }

    private func handleOutput(_ output: SpeechOutput) {
        switch output {
        case let .partial(text): emit(["event": "partial", "text": text])
        case let .final(text):
            guard closeDebugDump() else { return }
            terminalResultEmitted = true
            emit(["event": "final", "text": text])
        case let .error(kind, message):
            terminalResultEmitted = true
            emit(["event": "error", "kind": kind, "message": message])
        case .closed: close()
        }
    }

    @discardableResult
    private func closeDebugDump() -> Bool {
        guard let dump = debugWavDump else { return true }
        debugWavDump = nil
        do {
            let frames = try dump.close()
            diagnostic("event=debug-dump-closed frames=\(frames)")
            return true
        } catch {
            diagnostic("event=debug-dump-failed")
            if !terminalResultEmitted {
                terminalResultEmitted = true
                emit(["event": "error", "kind": "debug-dump", "message": error.localizedDescription])
            }
            return false
        }
    }

    func cancel() {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.terminal else { return }
            if let recognition = self.recognitionSession {
                recognition.cancel()
            } else {
                self.close()
            }
        }
    }

    private func fail(_ kind: String, _ message: String) {
        guard !terminal else { return }
        if let recognitionSession {
            recognitionSession.fail(kind, message)
        } else {
            terminalResultEmitted = true
            emit(["event": "error", "kind": kind, "message": message])
            close()
        }
    }

    private func close() {
        guard !terminal else { return }
        terminal = true
        earlyFinishDeadline?.cancel()
        debugInputPlayer?.stop()
        if audioEngine.isRunning { audioEngine.stop() }
        removeTap()
        closeDebugDump()
        emit(["event": "closed"])
        fflush(stdout)
        exit(0)
    }

    private func removeTap() {
        if tapInstalled {
            audioEngine.inputNode.removeTap(onBus: 0)
            tapInstalled = false
        }
    }
}

private struct Arguments {
    let locale: Locale
    let engine: SpeechEngine
    let inputDevice: String
    let debugInputWavPath: String?
    let debugDumpWavPath: String?
}

private func parseArguments() -> Arguments {
    var arguments = Array(CommandLine.arguments.dropFirst())
    let engine: SpeechEngine
    do {
        let selection: String
        if let index = arguments.firstIndex(of: "--engine") {
            guard index + 1 < arguments.count else {
                throw SpeechAnalysisFailure(kind: "arguments", message: "--engine の値を指定してください")
            }
            selection = arguments[index + 1]
            arguments.removeSubrange(index...index + 1)
        } else {
            selection = "auto"
        }
        engine = try SpeechEngine.resolve(selection)
    } catch let failure as SpeechAnalysisFailure {
        emit(["event": "error", "kind": failure.kind, "message": failure.message])
        exit(2)
    } catch {
        emit(["event": "error", "kind": "arguments", "message": "音声認識 engine を選択できませんでした"])
        exit(2)
    }
    guard (arguments.count == 4 || arguments.count == 6),
          arguments[0] == "--locale",
          arguments[2] == "--input-device" else {
        emit(["event": "error", "kind": "arguments", "message": "--locale と --input-device、任意で --debug-input-wav <path> または --debug-dump-wav <path> を指定してください"])
        exit(2)
    }
    let wavPath: String?
    let dumpPath: String?
    if arguments.count == 6 {
        guard !arguments[5].isEmpty else {
            emit(["event": "error", "kind": "arguments", "message": "WAV のパスを指定してください"])
            exit(2)
        }
        switch arguments[4] {
        case "--debug-input-wav" where arguments[3] == "default":
            wavPath = arguments[5]
            dumpPath = nil
        case "--debug-dump-wav" where URL(fileURLWithPath: arguments[5]).pathExtension.lowercased() == "wav":
            wavPath = nil
            dumpPath = arguments[5]
        default:
            emit(["event": "error", "kind": "arguments", "message": "--input-device default --debug-input-wav <path> または --debug-dump-wav <path.wav> を指定してください"])
            exit(2)
        }
    } else {
        wavPath = nil
        dumpPath = nil
    }
    let locale = arguments[1] == "system" ? Locale.current : Locale(identifier: arguments[1])
    return Arguments(locale: locale, engine: engine, inputDevice: arguments[3], debugInputWavPath: wavPath, debugDumpWavPath: dumpPath)
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

private let arguments = parseArguments()
private let session = SpeechSession(
    locale: arguments.locale, engine: arguments.engine, inputDevice: arguments.inputDevice,
    debugInputWavPath: arguments.debugInputWavPath, debugDumpWavPath: arguments.debugDumpWavPath
)
DispatchQueue.global(qos: .userInitiated).async {
    while let line = readLine() {
        guard let data = line.data(using: .utf8),
              let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let operation = value["op"] as? String else {
            continue
        }
        if operation == "finish" { session.finish() }
        if operation == "cancel" { session.cancel() }
    }
    session.cancel()
}
DispatchQueue.main.async { session.authorizeAndStart() }
RunLoop.main.run()
