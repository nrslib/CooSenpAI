import AVFoundation
import CoreAudio
import Foundation

protocol SpeakerAudioCapture: AnyObject {
    func start() throws -> AVAudioFormat
    func takeConfigurationChange() -> Bool
    func nextBuffer() throws -> AVAudioPCMBuffer?
    func stop() throws
    func close() throws
}

extension SpeakerAudioCapture {
    func close() throws { try stop() }
}

enum SpeakerAudioTapError: LocalizedError {
    case operation(String, OSStatus)
    case noOutputDevice
    case currentProcessUnavailable
    case invalidFormat
    case invalidBuffer
    case allocation
    case overflow
    case startupTimeout
    case noAudio
    case cleanup([String])
    case unexpected(Error)

    var kind: String {
        switch self {
        case .operation(_, kAudioDevicePermissionsError): return "system-audio-permission"
        case .noOutputDevice: return "system-audio-device"
        case .invalidFormat, .invalidBuffer: return "system-audio-format"
        case .overflow: return "system-audio-overflow"
        case .startupTimeout: return "system-audio-start-timeout"
        case .cleanup: return "system-audio-cleanup"
        default: return "system-audio"
        }
    }

    var errorDescription: String? {
        switch self {
        case .operation(_, kAudioDevicePermissionsError): return "システムオーディオ録音が許可されていません。システム設定の「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で許可し、Hearing を入れ直してください"
        case let .operation(operation, status): return "Core Audio operation=\(operation) status=\(status)"
        case .noOutputDevice: return "利用可能な音声出力デバイスがありません"
        case .currentProcessUnavailable: return "音声除外対象のプロセスを取得できませんでした"
        case .invalidFormat: return "スピーカー音声の形式を取得できませんでした"
        case .invalidBuffer: return "スピーカー音声のバッファ形式が変わりました"
        case .allocation: return "スピーカー音声のバッファを確保できませんでした"
        case .overflow: return "スピーカー音声の処理が追いつかず、バッファが上限に達しました"
        case .startupTimeout: return "スピーカー音声の開始処理が時間内に完了しませんでした"
        case .noAudio: return "再生中のスピーカー音声を受信できませんでした"
        case let .cleanup(details):
            return "Core Audio の後始末に失敗しました: \(details.joined(separator: "; "))"
        case let .unexpected(error): return error.localizedDescription
        }
    }
}

@available(macOS 14.2, *)
final class CoreAudioSpeakerDevice: SpeakerAudioCapture {
    // 32 HAL callbacks are buffered; an overrun is reported instead of silently losing speech.
    private static let frameCapacity: UInt32 = 16_384
    private let diagnostic: (String) -> Void
    private let onStage: (String, String) -> Void
    private let changes: OpaquePointer
    private var observedSequence: UInt64 = 0
    private var outputID = AudioObjectID(kAudioObjectUnknown)
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var deviceID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var ring: OpaquePointer?
    private var buffer: AVAudioPCMBuffer?
    private var listeners: [(AudioObjectID, AudioObjectPropertyAddress)] = []
    private var outputListenerInstalled = false
    private var deviceStarted = false
    private var cleanupFailed = false
    private var activityCheckAt = Date.distantPast
    private var missingAudioSince: Date?
    private var receivedCount: UInt64 = 0

    init(diagnostic: @escaping (String) -> Void, onStage: @escaping (String, String) -> Void) throws {
        self.diagnostic = diagnostic
        self.onStage = onStage
        guard let changes = coosenpai_audio_changes_create() else { throw SpeakerAudioTapError.allocation }
        self.changes = changes
        var address = Self.address(kAudioHardwarePropertyDefaultOutputDevice)
        let status = AudioObjectAddPropertyListener(
            AudioObjectID(kAudioObjectSystemObject), &address,
            coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes)
        )
        guard status == noErr else {
            throw SpeakerAudioTapError.operation("observe-default-output", status)
        }
        outputListenerInstalled = true
    }

    func start() throws -> AVAudioFormat {
        guard !cleanupFailed else { throw SpeakerAudioTapError.operation("previous-cleanup", kAudioHardwareUnspecifiedError) }
        observedSequence = coosenpai_audio_changes_sequence(changes)
        do {
            outputID = try uint32Property(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice)
            guard outputID != kAudioObjectUnknown,
                  try outputProperty(kAudioDevicePropertyDeviceIsAlive) != 0 else {
                throw SpeakerAudioTapError.noOutputDevice
            }
            onStage("tap-create", "begin")
            var address = Self.address(kAudioHardwarePropertyTranslatePIDToProcessObject)
            var pid = getpid()
            var processID = AudioObjectID(kAudioObjectUnknown)
            var size = UInt32(MemoryLayout<AudioObjectID>.size)
            try check(AudioObjectGetPropertyData(
                AudioObjectID(kAudioObjectSystemObject), &address,
                UInt32(MemoryLayout<pid_t>.size), &pid, &size, &processID
            ), "resolve-current-process")
            guard processID != kAudioObjectUnknown else { throw SpeakerAudioTapError.currentProcessUnavailable }
            let description = CATapDescription(stereoGlobalTapButExcludeProcesses: [processID])
            description.name = "CooSenpAI Hearing"
            description.isPrivate = true
            description.muteBehavior = .unmuted
            try check(AudioHardwareCreateProcessTap(description, &tapID), "create-tap")
            let tapUID = try readStringProperty(tapID, kAudioTapPropertyUID, operation: "read-tap-uid")
            onStage("tap-create", "end")
            onStage("aggregate-create", "begin")
            let deviceDescription: [String: Any] = [
                kAudioAggregateDeviceNameKey: "CooSenpAI Hearing",
                kAudioAggregateDeviceUIDKey: UUID().uuidString,
                kAudioAggregateDeviceIsPrivateKey: true,
                kAudioAggregateDeviceIsStackedKey: false,
                // Waiting for a tapped application can block AudioDeviceStart indefinitely.
                kAudioAggregateDeviceTapAutoStartKey: false,
            ]
            try check(AudioHardwareCreateAggregateDevice(deviceDescription as CFDictionary, &deviceID), "create-aggregate")
            onStage("aggregate-ready", "begin")
            try waitForDeviceAlive(deviceID)
            onStage("aggregate-ready", "end")
            try attachTap(tapUID, to: deviceID)
            var streamDescription = AudioStreamBasicDescription()
            address = Self.address(kAudioTapPropertyFormat)
            size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
            try check(AudioObjectGetPropertyData(tapID, &address, 0, nil, &size, &streamDescription), "read-tap-format")
            guard let format = AVAudioFormat(streamDescription: &streamDescription),
                  format.sampleRate > 0, format.channelCount > 0,
                  let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: Self.frameCapacity) else {
                throw SpeakerAudioTapError.invalidFormat
            }
            self.buffer = buffer
            guard let ring = coosenpai_audio_ring_create(&streamDescription, Self.frameCapacity, 32, changes) else {
                throw SpeakerAudioTapError.allocation
            }
            self.ring = ring
            onStage("aggregate-create", "end")
            onStage("add-output", "begin")
            try check(AudioDeviceCreateIOProcIDWithBlock(
                &ioProcID, deviceID, nil
            ) { _, inputData, _, _, _ in
                coosenpai_audio_ring_push(ring, inputData)
            }, "create-io-proc")
            onStage("add-output", "end")
            onStage("start-capture", "begin")
            try check(AudioDeviceStart(deviceID, ioProcID), "start-device")
            deviceStarted = true
            onStage("start-capture", "end")
            try observe(tapID, kAudioTapPropertyFormat)
            try observe(deviceID, kAudioDevicePropertyStreamFormat, scope: kAudioDevicePropertyScopeInput)
            for device in [deviceID, outputID] {
                try observe(device, kAudioDevicePropertyDeviceIsAlive)
                try observe(device, kAudioDevicePropertyNominalSampleRate)
                try observe(device, kAudioDevicePropertyBufferFrameSize)
            }
            missingAudioSince = nil
            activityCheckAt = .distantPast
            receivedCount = 0
            return format
        } catch {
            try stop()
            throw error
        }
    }

    func takeConfigurationChange() -> Bool {
        let sequence = coosenpai_audio_changes_sequence(changes)
        guard sequence != observedSequence else { return false }
        observedSequence = sequence
        return true
    }

    func nextBuffer() throws -> AVAudioPCMBuffer? {
        guard let ring, let buffer else { return nil }
        switch coosenpai_audio_ring_fault(ring) {
        case UInt32(COOSENPAI_AUDIO_RING_OVERFLOW): throw SpeakerAudioTapError.overflow
        case UInt32(COOSENPAI_AUDIO_RING_INVALID_LAYOUT): throw SpeakerAudioTapError.invalidBuffer
        default: break
        }
        buffer.frameLength = buffer.frameCapacity
        let frames = coosenpai_audio_ring_pop(ring, buffer.mutableAudioBufferList)
        if frames > 0 {
            buffer.frameLength = frames
            return buffer
        }
        try checkAudioDelivery(ring)
        return nil
    }

    private func checkAudioDelivery(_ ring: OpaquePointer) throws {
        let now = Date()
        guard now.timeIntervalSince(activityCheckAt) >= 0.5 else { return }
        activityCheckAt = now
        let count = coosenpai_audio_ring_received(ring)
        let playing = try outputProperty(kAudioDevicePropertyDeviceIsRunningSomewhere) != 0
        defer { receivedCount = count }
        guard playing, count == receivedCount else { missingAudioSince = nil; return }
        if let since = missingAudioSince {
            if now.timeIntervalSince(since) >= 3 { throw SpeakerAudioTapError.noAudio }
        } else { missingAudioSince = now }
    }

    func stop() throws {
        var failures: [String] = []
        func recordFailure(_ status: OSStatus, _ operation: String) {
            guard status != noErr else { return }
            diagnostic("audio-tap-cleanup operation=\(operation) status=\(status)")
            failures.append("operation=\(operation) status=\(status)")
        }

        var remainingListeners: [(AudioObjectID, AudioObjectPropertyAddress)] = []
        for (object, storedAddress) in listeners {
            var address = storedAddress
            let status = AudioObjectRemovePropertyListener(object, &address,
                coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
            // Removing the output device also removes its registered listeners.
            if status != noErr && status != kAudioHardwareBadObjectError {
                recordFailure(status, "remove-observer")
                remainingListeners.append((object, storedAddress))
            }
        }
        listeners = remainingListeners
        if let ioProcID {
            if deviceStarted {
                let status = AudioDeviceStop(deviceID, ioProcID)
                recordFailure(status, "stop-device")
                if status == noErr { deviceStarted = false }
            }
            let status = AudioDeviceDestroyIOProcID(deviceID, ioProcID)
            recordFailure(status, "destroy-io-proc")
            if status == noErr {
                self.ioProcID = nil
                deviceStarted = false
            }
        }
        // IOProc が残っている場合だけ callback の保存領域を残す。aggregate と tap の
        // 解除は別の資源なので、別の解除失敗があっても必ず試みる。
        if ioProcID == nil {
            if let ring {
                coosenpai_audio_ring_destroy(ring)
                self.ring = nil
            }
            buffer = nil
        } else {
            failures.append("operation=retain-callback-storage reason=io-proc-active")
        }
        if deviceID != kAudioObjectUnknown {
            let status = AudioHardwareDestroyAggregateDevice(deviceID)
            recordFailure(status, "destroy-aggregate")
            if status == noErr { deviceID = AudioObjectID(kAudioObjectUnknown) }
        }
        if tapID != kAudioObjectUnknown {
            let status = AudioHardwareDestroyProcessTap(tapID)
            recordFailure(status, "destroy-tap")
            if status == noErr { tapID = AudioObjectID(kAudioObjectUnknown) }
        }
        if !listeners.isEmpty { failures.append("operation=retain-observers") }
        cleanupFailed = !failures.isEmpty
            || ioProcID != nil
            || deviceID != kAudioObjectUnknown
            || tapID != kAudioObjectUnknown
        if cleanupFailed {
            if ioProcID != nil { failures.append("operation=retain-io-proc") }
            if deviceID != kAudioObjectUnknown { failures.append("operation=retain-aggregate") }
            if tapID != kAudioObjectUnknown { failures.append("operation=retain-tap") }
            throw SpeakerAudioTapError.cleanup(failures)
        }
    }

    func close() throws {
        var failures: [String] = []
        do {
            try stop()
        } catch {
            failures.append(error.localizedDescription)
        }
        if outputListenerInstalled {
            var address = Self.address(kAudioHardwarePropertyDefaultOutputDevice)
            let status = AudioObjectRemovePropertyListener(AudioObjectID(kAudioObjectSystemObject),
                &address, coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
            if status == noErr || status == kAudioHardwareBadObjectError {
                outputListenerInstalled = false
            } else {
                diagnostic("audio-tap-cleanup operation=remove-output-observer status=\(status)")
                failures.append("operation=remove-output-observer status=\(status)")
            }
        }
        if !failures.isEmpty {
            throw SpeakerAudioTapError.cleanup(failures)
        }
    }

    deinit {
        do { try close() }
        catch { diagnostic("audio-tap-cleanup operation=deinit detail=\(error.localizedDescription)") }
        if !outputListenerInstalled && listeners.isEmpty && ring == nil {
            coosenpai_audio_changes_destroy(changes)
        }
    }

    private func observe(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                         scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws {
        var address = Self.address(selector, scope: scope)
        let status = AudioObjectAddPropertyListener(object, &address,
            coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
        if object == outputID && (status == kAudioHardwareBadObjectError || status == kAudioHardwareBadDeviceError) {
            throw SpeakerAudioTapError.noOutputDevice
        }
        try check(status, "observe-device")
        listeners.append((object, address))
    }

    private func outputProperty(_ selector: AudioObjectPropertySelector) throws -> UInt32 {
        do { return try uint32Property(outputID, selector) }
        catch SpeakerAudioTapError.operation(_, kAudioHardwareBadObjectError) {
            throw SpeakerAudioTapError.noOutputDevice
        } catch SpeakerAudioTapError.operation(_, kAudioHardwareBadDeviceError) {
            throw SpeakerAudioTapError.noOutputDevice
        }
    }

    private func uint32Property(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> UInt32 {
        var address = Self.address(selector)
        var value: UInt32 = 0
        var size = UInt32(MemoryLayout<UInt32>.size)
        try check(AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value), "read-device-property")
        return value
    }

    private func readStringProperty(
        _ object: AudioObjectID,
        _ selector: AudioObjectPropertySelector,
        operation: String
    ) throws -> String {
        var address = Self.address(selector)
        var value: CFString = "" as CFString
        var size = UInt32(MemoryLayout<CFString>.stride)
        try check(
            withUnsafeMutablePointer(to: &value) { pointer in
                AudioObjectGetPropertyData(object, &address, 0, nil, &size, pointer)
            },
            operation
        )
        let string = value as String
        guard !string.isEmpty else { throw SpeakerAudioTapError.invalidFormat }
        return string
    }

    private func attachTap(_ uid: String, to aggregate: AudioObjectID) throws {
        var address = Self.address(kAudioAggregateDevicePropertyTapList)
        var size: UInt32 = 0
        try check(
            AudioObjectGetPropertyDataSize(aggregate, &address, 0, nil, &size),
            "read-tap-list-size"
        )
        var tapList: CFArray?
        if size > 0 {
            try check(
                withUnsafeMutablePointer(to: &tapList) { pointer in
                    AudioObjectGetPropertyData(aggregate, &address, 0, nil, &size, pointer)
                },
                "read-tap-list"
            )
        }
        var values = (tapList as? [CFString]) ?? []
        if !values.contains(uid as CFString) {
            values.append(uid as CFString)
            size += UInt32(MemoryLayout<CFString>.stride)
        }
        tapList = values as CFArray
        try check(
            withUnsafeMutablePointer(to: &tapList) { pointer in
                AudioObjectSetPropertyData(aggregate, &address, 0, nil, size, pointer)
            },
            "attach-tap"
        )
    }

    private func waitForDeviceAlive(_ device: AudioObjectID) throws {
        var address = Self.address(kAudioDevicePropertyDeviceIsAlive)
        for attempt in 0..<30 {
            var alive: UInt32 = 0
            var size = UInt32(MemoryLayout<UInt32>.size)
            try check(
                AudioObjectGetPropertyData(device, &address, 0, nil, &size, &alive),
                "wait-aggregate-alive"
            )
            if alive != 0 { return }
            if attempt < 29 { Thread.sleep(forTimeInterval: 0.1) }
        }
        throw SpeakerAudioTapError.startupTimeout
    }

    private static func address(_ selector: AudioObjectPropertySelector,
                                scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private func check(_ status: OSStatus, _ operation: String) throws {
        guard status == noErr else { throw SpeakerAudioTapError.operation(operation, status) }
    }

}
