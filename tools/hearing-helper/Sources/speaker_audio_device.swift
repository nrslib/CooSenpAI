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
    case unexpected(Error)

    var kind: String {
        switch self {
        case .operation(_, kAudioDevicePermissionsError): return "system-audio-permission"
        case .noOutputDevice: return "system-audio-device"
        case .invalidFormat, .invalidBuffer: return "system-audio-format"
        case .overflow: return "system-audio-overflow"
        case .startupTimeout: return "system-audio-start-timeout"
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
            onStage("tap-create", "end")
            onStage("aggregate-create", "begin")
            let deviceDescription: [String: Any] = [
                kAudioAggregateDeviceNameKey: "CooSenpAI Hearing",
                kAudioAggregateDeviceUIDKey: UUID().uuidString,
                kAudioAggregateDeviceIsPrivateKey: true,
                // Waiting for a tapped application can block AudioDeviceStart indefinitely.
                kAudioAggregateDeviceTapAutoStartKey: false,
                kAudioAggregateDeviceTapListKey: [[
                    kAudioSubTapUIDKey: description.uuid.uuidString,
                    kAudioSubTapDriftCompensationKey: true,
                ]],
            ]
            try check(AudioHardwareCreateAggregateDevice(deviceDescription as CFDictionary, &deviceID), "create-aggregate")
            var streamDescription = AudioStreamBasicDescription()
            address = Self.address(kAudioDevicePropertyStreamFormat, scope: kAudioDevicePropertyScopeInput)
            size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
            try check(AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, &streamDescription), "read-format")
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
            try check(AudioDeviceCreateIOProcID(
                deviceID, coosenpai_audio_ring_io_proc, UnsafeMutableRawPointer(ring), &ioProcID
            ), "create-io-proc")
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
        guard !cleanupFailed else { throw SpeakerAudioTapError.operation("previous-cleanup", kAudioHardwareUnspecifiedError) }
        listeners = listeners.filter { object, storedAddress in
            var address = storedAddress
            let status = AudioObjectRemovePropertyListener(object, &address,
                coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
            // Removing the output device also removes its registered listeners.
            if status == kAudioHardwareBadObjectError { return false }
            return !cleanup(status, "remove-observer")
        }
        if let ioProcID {
            if deviceStarted {
                let status = AudioDeviceStop(deviceID, ioProcID)
                guard cleanup(status, "stop-device") else {
                    cleanupFailed = true
                    throw SpeakerAudioTapError.operation("stop-device", status)
                }
            }
            if cleanup(AudioDeviceDestroyIOProcID(deviceID, ioProcID), "destroy-io-proc") {
                self.ioProcID = nil
                deviceStarted = false
            }
        }
        // A failed detach must never leave Core Audio pointing at freed callback storage.
        guard ioProcID == nil, listeners.isEmpty else {
            cleanupFailed = true
            throw SpeakerAudioTapError.operation("detach-callbacks", kAudioHardwareUnspecifiedError)
        }
        if let ring { coosenpai_audio_ring_destroy(ring); self.ring = nil }
        buffer = nil
        if deviceID != kAudioObjectUnknown,
           cleanup(AudioHardwareDestroyAggregateDevice(deviceID), "destroy-aggregate") {
            deviceID = AudioObjectID(kAudioObjectUnknown)
        }
        if deviceID == kAudioObjectUnknown, tapID != kAudioObjectUnknown,
           cleanup(AudioHardwareDestroyProcessTap(tapID), "destroy-tap") {
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
        cleanupFailed = deviceID != kAudioObjectUnknown || tapID != kAudioObjectUnknown
        if cleanupFailed { throw SpeakerAudioTapError.operation("destroy-capture", kAudioHardwareUnspecifiedError) }
    }

    func close() throws {
        try stop()
        if outputListenerInstalled {
            var address = Self.address(kAudioHardwarePropertyDefaultOutputDevice)
            try check(AudioObjectRemovePropertyListener(AudioObjectID(kAudioObjectSystemObject),
                &address, coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes)), "remove-output-observer")
            outputListenerInstalled = false
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

    private static func address(_ selector: AudioObjectPropertySelector,
                                scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    }

    private func check(_ status: OSStatus, _ operation: String) throws {
        guard status == noErr else { throw SpeakerAudioTapError.operation(operation, status) }
    }

    private func cleanup(_ status: OSStatus, _ operation: String) -> Bool {
        if status != noErr { diagnostic("audio-tap-cleanup operation=\(operation) status=\(status)") }
        return status == noErr
    }
}
