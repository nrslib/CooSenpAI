import AVFoundation
import AudioToolbox
import CoreAudio
import CoreAudioTypes
import Foundation

protocol SpeakerAudioCapture: AnyObject {
    var missingDeviceCount: Int { get }
    func start() throws -> AVAudioFormat
    func takeConfigurationChange() -> Bool
    func nextBuffer() throws -> SpeakerCapturedBuffer?
    func stop() throws
    func close() throws
}

struct SpeakerCapturedBuffer {
    let buffer: AVAudioPCMBuffer
    let timestamp: UInt64
}

extension SpeakerAudioCapture {
    var missingDeviceCount: Int { 0 }
    func close() throws { try stop() }
}

enum SpeakerAudioTapError: LocalizedError {
    case operation(String, OSStatus)
    case noOutputDevice
    case speakerDevicesMissing(Int)
    case outputDeviceListUnavailable
    case currentProcessUnavailable
    case invalidFormat
    case invalidBuffer
    case invalidTimestamp
    case allocation
    case overflow
    case startupTimeout
    case noAudio
    case cleanup([String])
    case unexpected(Error)

    func isRecoverableDeviceInterruption(hasStarted: Bool) -> Bool {
        switch self {
        case .speakerDevicesMissing, .outputDeviceListUnavailable: return true
        case .noOutputDevice: return hasStarted
        default: return false
        }
    }

    var kind: String {
        switch self {
        case .operation(_, kAudioDevicePermissionsError): return "system-audio-permission"
        case .noOutputDevice: return "system-audio-device"
        case .speakerDevicesMissing: return "speaker-device-missing"
        case .outputDeviceListUnavailable: return "system-audio-device-list"
        case .invalidFormat, .invalidBuffer: return "system-audio-format"
        case .invalidTimestamp: return "system-audio-clock"
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
        case let .speakerDevicesMissing(count): return "指定した出力デバイスのうち \(count) 台が未接続です"
        case .outputDeviceListUnavailable: return "出力デバイス一覧を取得できません"
        case .currentProcessUnavailable: return "音声除外対象のプロセスを取得できませんでした"
        case .invalidFormat: return "スピーカー音声の形式を取得できませんでした"
        case .invalidBuffer: return "スピーカー音声のバッファ形式が変わりました"
        case .invalidTimestamp: return "スピーカー音声の取得時刻を取得できませんでした"
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
protocol SpeakerSystemPropertyListening {
    func add(_ address: inout AudioObjectPropertyAddress, changes: OpaquePointer) -> OSStatus
    func remove(_ address: inout AudioObjectPropertyAddress, changes: OpaquePointer) -> OSStatus
}

@available(macOS 14.2, *)
private struct CoreAudioSystemPropertyListener: SpeakerSystemPropertyListening {
    func add(_ address: inout AudioObjectPropertyAddress, changes: OpaquePointer) -> OSStatus {
        AudioObjectAddPropertyListener(AudioObjectID(kAudioObjectSystemObject), &address,
            coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
    }

    func remove(_ address: inout AudioObjectPropertyAddress, changes: OpaquePointer) -> OSStatus {
        AudioObjectRemovePropertyListener(AudioObjectID(kAudioObjectSystemObject), &address,
            coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
    }
}

@available(macOS 14.2, *)
final class CoreAudioSpeakerDevice: SpeakerAudioCapture {
    // 32 HAL callbacks are buffered; an overrun is reported instead of silently losing speech.
    private static let frameCapacity: UInt32 = 16_384
    private let diagnostic: (String) -> Void
    private let onStage: (String, String) -> Void
    private let systemListener: any SpeakerSystemPropertyListening
    private let selectedUIDs: [String]
    private let changes: OpaquePointer
    private var observedSequence: UInt64 = 0
    private var observedDeviceListSequence: UInt64 = 0
    private var observedOutputDevices: [String: AudioObjectID] = [:]
    private var outputID = AudioObjectID(kAudioObjectUnknown)
    private var tapIDs: [AudioObjectID] = []
    private var deviceID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var ring: OpaquePointer?
    private var buffer: AVAudioPCMBuffer?
    private var listeners: [(AudioObjectID, AudioObjectPropertyAddress)] = []
    private var outputListenerInstalled = false
    private var deviceListListenerInstalled = false
    private var deviceStarted = false
    private var cleanupFailed = false
    private var activityCheckAt = Date.distantPast
    private var missingAudioSince: Date?
    private var receivedCount: UInt64 = 0
    private(set) var missingDeviceCount = 0
    private var selectedOutputIDs: [AudioObjectID] = []

    init(selectedUIDs: [String], diagnostic: @escaping (String) -> Void, onStage: @escaping (String, String) -> Void,
         systemListener: any SpeakerSystemPropertyListening = CoreAudioSystemPropertyListener()) throws {
        self.selectedUIDs = selectedUIDs
        self.diagnostic = diagnostic
        self.onStage = onStage
        self.systemListener = systemListener
        guard let changes = coosenpai_audio_changes_create() else { throw SpeakerAudioTapError.allocation }
        self.changes = changes
        var address: AudioObjectPropertyAddress
        if selectedUIDs.isEmpty {
            address = Self.address(kAudioHardwarePropertyDefaultOutputDevice)
            let status = systemListener.add(&address, changes: changes)
            guard status == noErr else {
                coosenpai_audio_changes_destroy(changes)
                throw SpeakerAudioTapError.operation("observe-default-output", status)
            }
            outputListenerInstalled = true
        } else {
            address = Self.address(kAudioHardwarePropertyDevices)
            do {
                try check(systemListener.add(&address, changes: changes), "observe-output-devices")
                deviceListListenerInstalled = true
            } catch {
                do { try close() }
                catch {
                    diagnostic("audio-tap-cleanup operation=init action=terminate-process")
                    exit(1)
                }
                coosenpai_audio_changes_destroy(changes)
                throw error
            }
        }
    }

    func start() throws -> AVAudioFormat {
        guard !cleanupFailed else { throw SpeakerAudioTapError.operation("previous-cleanup", kAudioHardwareUnspecifiedError) }
        selectedOutputIDs.removeAll()
        observedSequence = coosenpai_audio_changes_sequence(changes)
        observedDeviceListSequence = coosenpai_audio_device_list_sequence(changes)
        do {
            let tapTargets: [(uid: String, stream: UInt)]
            if selectedUIDs.isEmpty {
                outputID = try uint32Property(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice)
                guard outputID != kAudioObjectUnknown,
                      try outputProperty(kAudioDevicePropertyDeviceIsAlive) != 0 else {
                    throw SpeakerAudioTapError.noOutputDevice
                }
                tapTargets = []
                selectedOutputIDs = [outputID]
                missingDeviceCount = 0
            } else {
                let available: [SpeakerOutputDevice]
                do { available = try SpeakerOutputDevices.available() }
                catch { throw SpeakerAudioTapError.outputDeviceListUnavailable }
                observedOutputDevices = SpeakerOutputDevices.captureIdentity(available)
                let resolved = SpeakerOutputDevices.resolve(selectedUIDs, from: available)
                missingDeviceCount = resolved.missingCount
                guard !resolved.devices.isEmpty else { throw SpeakerAudioTapError.speakerDevicesMissing(missingDeviceCount) }
                tapTargets = SpeakerOutputDevices.captureStreams(resolved.devices)
                guard !tapTargets.isEmpty else { throw SpeakerAudioTapError.invalidFormat }
                selectedOutputIDs = resolved.devices.map(\.objectID)
                outputID = selectedOutputIDs[0]
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
            var tapUIDs: [String] = []
            let targets: [(uid: String, stream: UInt)?] = selectedUIDs.isEmpty ? [nil] : tapTargets.map(Optional.some)
            for target in targets {
                let description = target.map {
                    CATapDescription(excludingProcesses: [processID], deviceUID: $0.uid, stream: $0.stream)
                } ?? CATapDescription(stereoGlobalTapButExcludeProcesses: [processID])
                description.name = "CooSenpAI Hearing"
                description.isPrivate = true
                description.muteBehavior = .unmuted
                if target != nil {
                    description.isMixdown = true
                    description.isMono = true
                }
                var tapID = AudioObjectID(kAudioObjectUnknown)
                try check(AudioHardwareCreateProcessTap(description, &tapID), "create-tap")
                tapIDs.append(tapID)
                tapUIDs.append(try readStringProperty(tapID, kAudioTapPropertyUID, operation: "read-tap-uid"))
            }
            onStage("tap-create", "end")
            onStage("aggregate-create", "begin")
            let deviceDescription = Self.aggregateDescription(
                tapUIDs: tapUIDs,
                clockDeviceUID: tapTargets.first?.uid,
                tapDeviceUIDs: tapTargets.map(\.uid)
            )
            try check(AudioHardwareCreateAggregateDevice(deviceDescription as CFDictionary, &deviceID), "create-aggregate")
            onStage("aggregate-ready", "begin")
            try waitForDeviceAlive(deviceID)
            onStage("aggregate-ready", "end")
            if selectedUIDs.isEmpty { try attachTaps(tapUIDs, to: deviceID) }
            var streamDescription = AudioStreamBasicDescription()
            address = Self.address(kAudioTapPropertyFormat)
            size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
            try check(AudioObjectGetPropertyData(tapIDs[0],
                &address, 0, nil, &size, &streamDescription), "read-capture-format")
            var precedingInputChannels: UInt32 = 0
            if !selectedUIDs.isEmpty {
                let channels = try aggregateInputChannelCount(deviceID)
                guard channels >= tapIDs.count, streamDescription.mChannelsPerFrame == 1 else {
                    throw SpeakerAudioTapError.invalidFormat
                }
                precedingInputChannels = UInt32(channels - tapIDs.count)
                streamDescription.mSampleRate = try doubleProperty(deviceID, kAudioDevicePropertyNominalSampleRate)
                streamDescription.mChannelsPerFrame = UInt32(tapIDs.count)
                streamDescription.mFormatFlags |= kAudioFormatFlagIsNonInterleaved
            }
            let format: AVAudioFormat?
            if streamDescription.mChannelsPerFrame > 2 {
                let layout = AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | streamDescription.mChannelsPerFrame)
                if let layout {
                    format = AVAudioFormat(streamDescription: &streamDescription, channelLayout: layout)
                } else {
                    format = nil
                }
            } else {
                format = AVAudioFormat(streamDescription: &streamDescription)
            }
            guard let format,
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
            let tapSuffixOnly = !selectedUIDs.isEmpty
            try check(AudioDeviceCreateIOProcIDWithBlock(
                &ioProcID, deviceID, nil
            ) { _, inputData, inputTime, _, _ in
                let time = inputTime.pointee
                let hostTime = time.mFlags.contains(.hostTimeValid) ? time.mHostTime : 0
                if tapSuffixOnly {
                    // The clock subdevice's input buffers precede the aggregate's tap buffers.
                    coosenpai_audio_ring_push_tap_suffix(ring, inputData, hostTime, precedingInputChannels)
                } else {
                    coosenpai_audio_ring_push(ring, inputData, hostTime)
                }
            }, "create-io-proc")
            onStage("add-output", "end")
            onStage("start-capture", "begin")
            try check(AudioDeviceStart(deviceID, ioProcID), "start-device")
            deviceStarted = true
            onStage("start-capture", "end")
            for tapID in tapIDs { try observe(tapID, kAudioTapPropertyFormat) }
            for address in Self.aggregateMonitorAddresses() {
                try observe(deviceID, address.mSelector, scope: address.mScope)
            }
            for device in selectedOutputIDs {
                try observe(device, kAudioDevicePropertyDeviceIsAlive)
                try observe(device, kAudioDevicePropertyNominalSampleRate)
                try observe(device, kAudioDevicePropertyBufferFrameSize)
                if !selectedUIDs.isEmpty {
                    try observe(device, kAudioDevicePropertyStreamConfiguration, scope: kAudioDevicePropertyScopeOutput)
                }
            }
            missingAudioSince = nil
            activityCheckAt = .distantPast
            receivedCount = 0
            return format
        } catch {
            let failure = classifyCaptureFailure(error)
            try stop()
            throw failure
        }
    }

    private func classifyCaptureFailure(_ error: Error) -> Error {
        guard case let SpeakerAudioTapError.operation(_, status) = error,
              status == kAudioHardwareBadObjectError || status == kAudioHardwareBadDeviceError
        else { return error }
        guard let currentIDs = try? SpeakerOutputDevices.objectIDs() else {
            return SpeakerAudioTapError.outputDeviceListUnavailable
        }
        let missing = selectedOutputIDs.contains {
            !currentIDs.contains($0) || (try? uint32Property($0, kAudioDevicePropertyDeviceIsAlive)) == 0
        }
        return Self.deviceDisappearanceError(status: status, selected: !selectedUIDs.isEmpty, missing: missing) ?? error
    }

    static func deviceDisappearanceError(status: OSStatus, selected: Bool, missing: Bool) -> SpeakerAudioTapError? {
        guard missing, status == kAudioHardwareBadObjectError || status == kAudioHardwareBadDeviceError else { return nil }
        return selected ? .speakerDevicesMissing(1) : .noOutputDevice
    }

    static func cleanupReleased(_ status: OSStatus) -> Bool {
        status == noErr || status == kAudioHardwareBadObjectError || status == kAudioHardwareBadDeviceError
    }

    static func aggregateMonitorAddresses() -> [AudioObjectPropertyAddress] {
        [
            address(kAudioDevicePropertyStreamFormat, scope: kAudioDevicePropertyScopeInput),
            address(kAudioDevicePropertyDeviceIsAlive),
            address(kAudioDevicePropertyNominalSampleRate),
            address(kAudioDevicePropertyBufferFrameSize),
        ]
    }

    static func aggregateDescription(
        tapUIDs: [String], clockDeviceUID: String?, tapDeviceUIDs: [String]
    ) -> [String: Any] {
        var description: [String: Any] = [
            kAudioAggregateDeviceNameKey: "CooSenpAI Hearing",
            kAudioAggregateDeviceUIDKey: SpeakerOutputDevices.aggregateUIDPrefix + UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            // Waiting for a tapped application can block AudioDeviceStart indefinitely.
            kAudioAggregateDeviceTapAutoStartKey: false,
        ]
        if let clockDeviceUID {
            precondition(tapUIDs.count == tapDeviceUIDs.count && !tapUIDs.isEmpty)
            description[kAudioAggregateDeviceMainSubDeviceKey] = clockDeviceUID
            description[kAudioAggregateDeviceSubDeviceListKey] = [[kAudioSubDeviceUIDKey: clockDeviceUID]]
            description[kAudioAggregateDeviceTapListKey] = zip(tapUIDs, tapDeviceUIDs).map { tapUID, deviceUID in
                [
                    kAudioSubTapUIDKey: tapUID,
                    kAudioSubTapDriftCompensationKey: deviceUID != clockDeviceUID,
                ] as [String: Any]
            }
        }
        return description
    }

    func takeConfigurationChange() -> Bool {
        let sequence = coosenpai_audio_changes_sequence(changes)
        let listSequence = coosenpai_audio_device_list_sequence(changes)
        let propertyChanged = sequence != observedSequence
        let listChanged = listSequence != observedDeviceListSequence
        guard propertyChanged || listChanged else { return false }
        observedSequence = sequence
        observedDeviceListSequence = listSequence
        guard listChanged, !selectedUIDs.isEmpty else { return propertyChanged }
        guard let available = try? SpeakerOutputDevices.available() else { return true }
        let outputDevices = SpeakerOutputDevices.captureIdentity(available)
        let changed = outputDevices != observedOutputDevices
        observedOutputDevices = outputDevices
        return propertyChanged || changed
    }

    func nextBuffer() throws -> SpeakerCapturedBuffer? {
        guard let ring, let buffer else { return nil }
        switch coosenpai_audio_ring_fault(ring) {
        case UInt32(COOSENPAI_AUDIO_RING_OVERFLOW): throw SpeakerAudioTapError.overflow
        case UInt32(COOSENPAI_AUDIO_RING_INVALID_LAYOUT): throw SpeakerAudioTapError.invalidBuffer
        case UInt32(COOSENPAI_AUDIO_RING_INVALID_TIMESTAMP): throw SpeakerAudioTapError.invalidTimestamp
        default: break
        }
        buffer.frameLength = buffer.frameCapacity
        var hostTime: UInt64 = 0
        let frames = coosenpai_audio_ring_pop(ring, buffer.mutableAudioBufferList, &hostTime)
        if frames > 0 {
            buffer.frameLength = frames
            return SpeakerCapturedBuffer(buffer: buffer, timestamp: AudioConvertHostTimeToNanos(hostTime))
        }
        try checkAudioDelivery(ring)
        return nil
    }

    private func checkAudioDelivery(_ ring: OpaquePointer) throws {
        let now = Date()
        guard now.timeIntervalSince(activityCheckAt) >= 0.5 else { return }
        activityCheckAt = now
        let count = coosenpai_audio_ring_received(ring)
        let playing = try selectedOutputIDs.contains { device in
            do { return try uint32Property(device, kAudioDevicePropertyDeviceIsRunningSomewhere) != 0 }
            catch SpeakerAudioTapError.operation(_, kAudioHardwareBadObjectError) {
                throw selectedUIDs.isEmpty ? SpeakerAudioTapError.noOutputDevice : .speakerDevicesMissing(1)
            } catch SpeakerAudioTapError.operation(_, kAudioHardwareBadDeviceError) {
                throw selectedUIDs.isEmpty ? SpeakerAudioTapError.noOutputDevice : .speakerDevicesMissing(1)
            }
        }
        defer { receivedCount = count }
        guard playing, count == receivedCount else { missingAudioSince = nil; return }
        if let since = missingAudioSince {
            if now.timeIntervalSince(since) >= 3 { throw SpeakerAudioTapError.noAudio }
        } else { missingAudioSince = now }
    }

    func stop() throws {
        var failures: [String] = []
        func recordFailure(_ status: OSStatus, _ operation: String) {
            guard !Self.cleanupReleased(status) else { return }
            diagnostic("audio-tap-cleanup operation=\(operation) status=\(status)")
            failures.append("operation=\(operation) status=\(status)")
        }

        var remainingListeners: [(AudioObjectID, AudioObjectPropertyAddress)] = []
        for (object, storedAddress) in listeners {
            var address = storedAddress
            let status = AudioObjectRemovePropertyListener(object, &address,
                coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
            if !Self.cleanupReleased(status) {
                recordFailure(status, "remove-observer")
                remainingListeners.append((object, storedAddress))
            }
        }
        listeners = remainingListeners
        if let ioProcID {
            if deviceStarted {
                let status = AudioDeviceStop(deviceID, ioProcID)
                recordFailure(status, "stop-device")
                if Self.cleanupReleased(status) { deviceStarted = false }
            }
            let status = AudioDeviceDestroyIOProcID(deviceID, ioProcID)
            recordFailure(status, "destroy-io-proc")
            if Self.cleanupReleased(status) {
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
            if Self.cleanupReleased(status) { deviceID = AudioObjectID(kAudioObjectUnknown) }
        }
        var remainingTapIDs: [AudioObjectID] = []
        for tapID in tapIDs {
            let status = AudioHardwareDestroyProcessTap(tapID)
            recordFailure(status, "destroy-tap")
            if !Self.cleanupReleased(status) { remainingTapIDs.append(tapID) }
        }
        tapIDs = remainingTapIDs
        if !listeners.isEmpty { failures.append("operation=retain-observers") }
        cleanupFailed = !failures.isEmpty
            || ioProcID != nil
            || deviceID != kAudioObjectUnknown
            || !tapIDs.isEmpty
        if cleanupFailed {
            if ioProcID != nil { failures.append("operation=retain-io-proc") }
            if deviceID != kAudioObjectUnknown { failures.append("operation=retain-aggregate") }
            if !tapIDs.isEmpty { failures.append("operation=retain-tap") }
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
            let status = systemListener.remove(&address, changes: changes)
            if Self.cleanupReleased(status) {
                outputListenerInstalled = false
            } else {
                diagnostic("audio-tap-cleanup operation=remove-output-observer status=\(status)")
                failures.append("operation=remove-output-observer status=\(status)")
            }
        }
        if deviceListListenerInstalled {
            var address = Self.address(kAudioHardwarePropertyDevices)
            let status = systemListener.remove(&address, changes: changes)
            if Self.cleanupReleased(status) {
                deviceListListenerInstalled = false
            } else {
                diagnostic("audio-tap-cleanup operation=remove-device-list-observer status=\(status)")
                failures.append("operation=remove-device-list-observer status=\(status)")
            }
        }
        if !failures.isEmpty {
            throw SpeakerAudioTapError.cleanup(failures)
        }
    }

    deinit {
        do { try close() }
        catch { diagnostic("audio-tap-cleanup operation=deinit detail=\(error.localizedDescription)") }
        if !outputListenerInstalled && !deviceListListenerInstalled && listeners.isEmpty && ring == nil {
            coosenpai_audio_changes_destroy(changes)
        }
    }

    private func observe(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                         scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws {
        var address = Self.address(selector, scope: scope)
        let status = AudioObjectAddPropertyListener(object, &address,
            coosenpai_audio_property_changed, UnsafeMutableRawPointer(changes))
        if selectedOutputIDs.contains(object) && (status == kAudioHardwareBadObjectError || status == kAudioHardwareBadDeviceError) {
            throw selectedUIDs.isEmpty ? SpeakerAudioTapError.noOutputDevice : .speakerDevicesMissing(1)
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

    private func doubleProperty(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) throws -> Double {
        var address = Self.address(selector)
        var value: Double = 0
        var size = UInt32(MemoryLayout<Double>.size)
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

    private func attachTaps(_ uids: [String], to aggregate: AudioObjectID) throws {
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
        for uid in uids where !values.contains(uid as CFString) {
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

    private func aggregateInputChannelCount(_ aggregate: AudioObjectID) throws -> Int {
        var address = Self.address(kAudioDevicePropertyStreamConfiguration, scope: kAudioDevicePropertyScopeInput)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(aggregate, &address, 0, nil, &size), "read-aggregate-channels-size")
        guard size >= MemoryLayout<AudioBufferList>.size else { throw SpeakerAudioTapError.invalidFormat }
        let storage = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { storage.deallocate() }
        let list = storage.bindMemory(to: AudioBufferList.self, capacity: 1)
        try check(AudioObjectGetPropertyData(aggregate, &address, 0, nil, &size, list), "read-aggregate-channels")
        let buffers = UnsafeMutableAudioBufferListPointer(list)
        let channels = buffers.reduce(0) { $0 + Int($1.mNumberChannels) }
        guard channels > 0 else { throw SpeakerAudioTapError.invalidFormat }
        return channels
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
