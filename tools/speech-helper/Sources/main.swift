import AVFoundation
import AudioToolbox
import CoreAudio
import Foundation
import Speech

private let outputLock = NSLock()

private func emit(_ value: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: value) else { return }
    outputLock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
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

private final class SpeechSession: @unchecked Sendable {
    private let locale: Locale
    private let inputDevice: String
    private let audioEngine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var latestText = ""
    private var terminal = false
    private var tapInstalled = false
    private var started = false
    private var finishRequested = false
    private var finalizing = false
    private var finalEmitted = false

    init(locale: Locale, inputDevice: String) {
        self.locale = locale
        self.inputDevice = inputDevice
    }

    func authorizeAndStart() {
        requestMicrophone { [weak self] microphone in
            DispatchQueue.main.async {
                self?.handleMicrophoneAuthorization(microphone)
            }
        }
    }

    private func handleMicrophoneAuthorization(_ microphone: AVAuthorizationStatus) {
        guard !terminal else { return }
        guard microphone == .authorized else {
            fail("permission-microphone", "マイクの使用が許可されていません")
            return
        }
        requestRecognition { [weak self] recognition in
            DispatchQueue.main.async {
                self?.handleRecognitionAuthorization(microphone, recognition)
            }
        }
    }

    private func handleRecognitionAuthorization(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !terminal else { return }
        guard recognition == .authorized else {
            fail("permission-speech", "音声認識の使用が許可されていません")
            return
        }
        startRecognition(microphone, recognition)
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

    private func requestRecognition(
        _ completion: @escaping @Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void
    ) {
        let status = SFSpeechRecognizer.authorizationStatus()
        guard status == .notDetermined else {
            completion(status)
            return
        }
        SFSpeechRecognizer.requestAuthorization(completion)
    }

    private func startRecognition(
        _ microphone: AVAuthorizationStatus,
        _ recognition: SFSpeechRecognizerAuthorizationStatus
    ) {
        guard !terminal else { return }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            fail("locale-unavailable", "指定したロケールの音声認識は利用できません: \(locale.identifier)")
            return
        }
        guard recognizer.supportsOnDeviceRecognition else {
            fail("on-device-unsupported", "指定したロケールはオンデバイス音声認識に対応していません: \(locale.identifier)")
            return
        }
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = true
        self.request = request
        let input = audioEngine.inputNode
        if inputDevice != "default" {
            do {
                try selectInputDevice(inputDevice, inputNode: input)
            } catch {
                do {
                    try useDefaultInputDevice(inputNode: input)
                } catch {
                    fail("input-device", "選択したマイクとシステム既定のマイクを利用できません")
                    return
                }
                emit([
                    "event": "warning",
                    "kind": "input-device-fallback",
                    "message": "選択したマイクを利用できないため、システム既定を使います",
                ])
            }
        }
        let format = input.outputFormat(forBus: 0)
        input.installTap(onBus: 0, bufferSize: 1_024, format: format) { buffer, _ in
            request.append(buffer)
        }
        tapInstalled = true
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            DispatchQueue.main.async {
                self?.handleRecognitionResult(result, error: error)
            }
        }
        do {
            audioEngine.prepare()
            try audioEngine.start()
            started = true
            emit([
                "event": "ready",
                "locale": locale.identifier,
                "microphone": permissionName(microphone),
                "recognition": permissionName(recognition),
            ])
            beginFinalizationIfRequested()
        } catch {
            fail("audio", error.localizedDescription)
        }
    }

    private func handleRecognitionResult(_ result: SFSpeechRecognitionResult?, error: Error?) {
        guard !terminal else { return }
        if let result {
            let text = result.bestTranscription.formattedString
            if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                latestText = text
            }
            if result.isFinal {
                emitFinal()
                close()
            } else {
                emit(["event": "partial", "text": text])
            }
        } else if let error {
            fail("recognition", error.localizedDescription)
        }
    }

    func finish() {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.terminal else { return }
            self.finishRequested = true
            self.beginFinalizationIfRequested()
        }
    }

    private func beginFinalizationIfRequested() {
        guard finishRequested, started, !finalizing, !terminal else { return }
        finalizing = true
        finishRecording()
        DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, !self.terminal else { return }
            if self.latestText.isEmpty {
                self.fail("no-speech", "音声を認識できませんでした")
            } else {
                self.emitFinal()
                self.close()
            }
        }
    }

    private func finishRecording() {
        audioEngine.stop()
        removeTap()
        request?.endAudio()
    }

    private func emitFinal() {
        guard !finalEmitted else { return }
        finalEmitted = true
        emit(["event": "final", "text": latestText])
    }

    func cancel() {
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.terminal else { return }
            self.audioEngine.stop()
            self.removeTap()
            self.request?.endAudio()
            self.task?.cancel()
            self.close()
        }
    }

    private func fail(_ kind: String, _ message: String) {
        guard !terminal else { return }
        emit(["event": "error", "kind": kind, "message": message])
        close()
    }

    private func close() {
        guard !terminal else { return }
        terminal = true
        if audioEngine.isRunning { audioEngine.stop() }
        removeTap()
        request?.endAudio()
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
    let inputDevice: String
}

private func parseArguments() -> Arguments {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.count == 4,
          arguments[0] == "--locale",
          arguments[2] == "--input-device" else {
        emit(["event": "error", "kind": "arguments", "message": "--locale と --input-device を指定してください"])
        exit(2)
    }
    let locale = arguments[1] == "system" ? Locale.current : Locale(identifier: arguments[1])
    return Arguments(locale: locale, inputDevice: arguments[3])
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
private let session = SpeechSession(locale: arguments.locale, inputDevice: arguments.inputDevice)
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
