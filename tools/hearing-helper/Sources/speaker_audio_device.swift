import AVFoundation
import Darwin
import AudioToolbox
import CoreAudio
import CoreAudioTypes
import Foundation

func speakerExcludedProcessObjects(current: AudioObjectID, additional: [AudioObjectID]) -> [AudioObjectID] {
    var seen: Set<AudioObjectID> = []
    return ([current] + additional).filter { $0 != kAudioObjectUnknown && seen.insert($0).inserted }
}

struct SpeakerProcessExclusions {
    let objects: [AudioObjectID]
    let appliedPIDs: Set<pid_t>
    let failedPIDs: Set<pid_t>

    static func resolve(
        requested: Set<pid_t>, currentPID: pid_t, currentObject: AudioObjectID,
        processObject: (pid_t) -> AudioObjectID?
    ) -> Self {
        var objects: [AudioObjectID] = []
        var applied: Set<pid_t> = []
        var failed: Set<pid_t> = []
        for pid in requested where pid != currentPID {
            if let object = processObject(pid), object != kAudioObjectUnknown {
                objects.append(object)
                applied.insert(pid)
            } else {
                failed.insert(pid)
            }
        }
        return Self(objects: speakerExcludedProcessObjects(current: currentObject, additional: objects),
                    appliedPIDs: applied, failedPIDs: failed)
    }

    func needsRebuild(resolved: Self) -> Bool {
        Set(objects) != Set(resolved.objects)
    }
}

func isCurrentSiblingHelper(_ pid: pid_t, parentPID: pid_t) -> Bool {
    guard pid > 1, pid != getpid() else { return false }
    var ownPath = [CChar](repeating: 0, count: 4_096)
    var candidatePath = [CChar](repeating: 0, count: 4_096)
    guard proc_pidpath(getpid(), &ownPath, UInt32(ownPath.count)) > 0,
          proc_pidpath(pid, &candidatePath, UInt32(candidatePath.count)) > 0 else { return false }
    var info = proc_bsdinfo()
    let size = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout<proc_bsdinfo>.size))
    return size == MemoryLayout<proc_bsdinfo>.size
        && info.pbi_ppid == parentPID
        && String(cString: candidatePath) == String(cString: ownPath)
}

func siblingHearingPIDs(
    parentPID: pid_t,
    diagnostic: (String) -> Void,
    listChildren: (pid_t, UnsafeMutableRawPointer?, Int32) -> Int32 = proc_listchildpids
) -> Set<pid_t>? {
    let requiredCount = listChildren(parentPID, nil, 0)
    guard requiredCount >= 0 else {
        diagnostic("audio-tap-exclusion child-pids=0 status=\(requiredCount)")
        return nil
    }
    if requiredCount == 0 {
        diagnostic("audio-tap-exclusion child-pids=0 matched=0")
        return []
    }
    let capacity = Int(requiredCount) + 8
    var children = [pid_t](repeating: 0, count: capacity)
    let count = listChildren(parentPID, &children, Int32(capacity * MemoryLayout<pid_t>.stride))
    guard count >= 0, Int(count) <= capacity else {
        diagnostic("audio-tap-exclusion child-pids=0 status=\(count)")
        return nil
    }
    let siblings = Set(children.prefix(Int(count)).filter {
        isCurrentSiblingHelper($0, parentPID: parentPID)
    })
    diagnostic("audio-tap-exclusion child-pids=\(count) matched=\(siblings.count)")
    return siblings
}

func speakerCurrentExclusionPIDs(
    configured: [pid_t], parentPID: pid_t?, siblings: Set<pid_t>,
    isSibling: (pid_t, pid_t) -> Bool
) -> Set<pid_t> {
    guard let parentPID else { return [] }
    let verified = configured.filter { isSibling($0, parentPID) }
    return Set([parentPID]).union(siblings).union(verified)
}

protocol SpeakerAudioCapture: AnyObject {
    var missingDeviceCount: Int { get }
    func start(onPrepared: (AVAudioFormat, SpeakerBufferCapacity) throws -> Void,
               onCapturing: () -> Void) throws -> AVAudioFormat
    func takeConfigurationChange() throws -> String?
    func nextBuffer(checkDelivery: Bool) throws -> SpeakerCapturedBuffer?
    func stop() throws
    func close() throws
    func updateExcludedProcessPIDs(_ pids: [pid_t])
}

struct SpeakerCapturedBuffer {
    let buffer: AVAudioPCMBuffer
    let timestamp: UInt64
}

struct SpeakerBufferCapacity {
    static let durationSeconds = 2.0
    static let maximumSlots = 8_192
    // PCM とリングの既知の確保量を 60 MiB に抑え、残り 4 MiB を AVAudioPCMBuffer 等の管理領域に残す。
    static let maximumPreallocatedBytes: UInt64 = 64 * 1_024 * 1_024
    private static let allocationReserveBytes: UInt64 = 4 * 1_024 * 1_024
    let slotCount: UInt32
    let frameCapacity: AVAudioFrameCount
    let preallocatedBytes: UInt64

    init(sampleRate: Double, callbackFrames: UInt32,
         ringBytesPerFrame: UInt64, ringBufferCount: UInt64,
         outputBytesPerFrame: UInt64, outputBufferCount: UInt64) throws {
        guard sampleRate.isFinite, sampleRate > 0,
              callbackFrames > 0, callbackFrames <= 16_384,
              ringBytesPerFrame > 0, ringBufferCount > 0,
              outputBytesPerFrame > 0, outputBufferCount > 0 else { throw SpeakerAudioTapError.invalidFormat }
        let slots = ceil(sampleRate * Self.durationSeconds / Double(callbackFrames))
        guard slots.isFinite, slots >= 1 else {
            throw SpeakerAudioTapError.invalidFormat
        }
        let slotsWithinCountLimit = Int(min(Double(Self.maximumSlots), max(2, slots)))
        let frames = UInt64(min(16_384, max(1_024, Int(callbackFrames) * 8)))
        let (ringFrameBytes, ringOverflow) = ringBytesPerFrame.multipliedReportingOverflow(by: ringBufferCount)
        let (outputFrameBytes, outputOverflow) = outputBytesPerFrame.multipliedReportingOverflow(by: outputBufferCount)
        let (combinedFrameBytes, combinedOverflow) = ringFrameBytes.addingReportingOverflow(outputFrameBytes)
        let (slotPCMBytes, slotOverflow) = combinedFrameBytes.multipliedReportingOverflow(by: frames)
        let (outputBufferBytes, bufferOverflow) = outputFrameBytes.multipliedReportingOverflow(by: frames)
        guard !ringOverflow, !outputOverflow, !combinedOverflow, !slotOverflow, !bufferOverflow,
              slotPCMBytes <= UInt64.max - 12 else { throw SpeakerAudioTapError.capacityLimit }
        let slotBytes = slotPCMBytes + 12 // ring の frameLength と timestamp
        let budget = Self.maximumPreallocatedBytes - Self.allocationReserveBytes
        guard outputBufferBytes < budget else { throw SpeakerAudioTapError.capacityLimit }
        let boundedSlots = min(slotsWithinCountLimit, Int((budget - outputBufferBytes) / slotBytes))
        guard boundedSlots >= 2 else { throw SpeakerAudioTapError.capacityLimit }
        slotCount = UInt32(boundedSlots)
        frameCapacity = AVAudioFrameCount(frames)
        preallocatedBytes = outputBufferBytes + UInt64(boundedSlots) * slotBytes
    }
}

struct SpeakerAggregateInputLayout {
    static func ringFrameBytes(bufferChannels: [UInt32]) throws -> UInt64 {
        let channels = bufferChannels.reduce(UInt64(0)) { $0 + UInt64($1) }
        guard channels > 0, channels <= UInt64(UInt32.max) else { throw SpeakerAudioTapError.invalidFormat }
        // tap_mix は入力の配置に関係なく、全チャンネルを Float32 として保存する。
        return channels * UInt64(MemoryLayout<Float>.size)
    }

    static func tapChannels(
        tapFormats: [AudioStreamBasicDescription],
        aggregateFormat: AudioStreamBasicDescription,
        bufferChannels: [UInt32],
        nominalRate: Double
    ) throws -> [UInt32] {
        func unsupported(_ reason: String) -> SpeakerAudioTapError {
            .unsupportedAggregateInput(reason)
        }
        guard !tapFormats.isEmpty, !bufferChannels.isEmpty, bufferChannels.allSatisfy({ $0 > 0 }) else {
            throw unsupported("tap または入力バッファがありません")
        }
        let requiredFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked
        guard aggregateFormat.mFormatID == kAudioFormatLinearPCM,
              aggregateFormat.mFormatFlags & requiredFlags == requiredFlags,
              aggregateFormat.mFormatFlags & kAudioFormatFlagIsBigEndian == 0,
              aggregateFormat.mBitsPerChannel == 32,
              aggregateFormat.mFramesPerPacket == 1,
              aggregateFormat.mChannelsPerFrame > 0,
              aggregateFormat.mBytesPerFrame > 0,
              aggregateFormat.mBytesPerPacket == aggregateFormat.mBytesPerFrame else {
            throw unsupported("集約入力が Float32 PCM ではありません")
        }
        let expectedFrameBytes = aggregateFormat.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
            ? UInt64(4) : UInt64(aggregateFormat.mChannelsPerFrame) * 4
        guard UInt64(aggregateFormat.mBytesPerFrame) == expectedFrameBytes else {
            throw unsupported("集約入力の 1 フレームのバイト数がチャンネル数と一致しません")
        }
        guard aggregateFormat.mSampleRate > 0, aggregateFormat.mSampleRate == nominalRate else {
            throw unsupported("集約入力のサンプルレートがデバイスの公称値と一致しません")
        }
        guard tapFormats.allSatisfy({ $0.mFormatID == kAudioFormatLinearPCM && $0.mChannelsPerFrame > 0 }) else {
            throw unsupported("tap の PCM 形式を取得できません")
        }
        let channels = bufferChannels.reduce(UInt64(0)) { $0 + UInt64($1) }
        guard channels <= UInt64(UInt32.max) else { throw unsupported("入力チャンネル数が上限を超えました") }
        let advertised = tapFormats.map(\.mChannelsPerFrame)
        if advertised.reduce(UInt64(0), { $0 + UInt64($1) }) == channels { return advertised }
        if bufferChannels.count == tapFormats.count && Set(bufferChannels).count > 1 {
            throw unsupported("tap ごとの入力チャンネル数を特定できません")
        }
        guard Set(advertised).count == 1, channels % UInt64(tapFormats.count) == 0 else {
            throw unsupported("tap ごとの入力チャンネル数を特定できません")
        }
        let width = channels / UInt64(tapFormats.count)
        guard width > 0, width <= UInt64(UInt32.max) else {
            throw unsupported("tap ごとの入力チャンネル数が不正です")
        }
        return Array(repeating: UInt32(width), count: tapFormats.count)
    }
}

struct SpeakerConfigurationSnapshot: Equatable {
    let inputFormats: [String]
    let aggregateInputChannels: [UInt32]
    let aggregateRate: Double
    let aggregateBufferFrames: UInt32
    let aggregateAlive: UInt32
    let outputIDs: [AudioObjectID]
    let outputAlive: [UInt32]
    let outputRates: [Double]
    let outputBufferFrames: [UInt32]
    let outputStreams: [[UInt]]

    func changes(from previous: Self) -> [String] {
        var changes: [String] = []
        if inputFormats != previous.inputFormats {
            changes.append("input-format=\(previous.inputFormats)->\(inputFormats)")
        }
        if aggregateInputChannels != previous.aggregateInputChannels {
            changes.append("input-channels=\(previous.aggregateInputChannels)->\(aggregateInputChannels)")
        }
        if aggregateRate != previous.aggregateRate {
            changes.append("aggregate-rate=\(previous.aggregateRate)->\(aggregateRate)")
        }
        if aggregateBufferFrames != previous.aggregateBufferFrames {
            changes.append("aggregate-buffer-frames=\(previous.aggregateBufferFrames)->\(aggregateBufferFrames)")
        }
        if aggregateAlive != previous.aggregateAlive {
            changes.append("aggregate-alive=\(previous.aggregateAlive)->\(aggregateAlive)")
        }
        if outputIDs != previous.outputIDs {
            changes.append("output-ids=\(previous.outputIDs)->\(outputIDs)")
        }
        if outputAlive != previous.outputAlive {
            changes.append("output-alive=\(previous.outputAlive)->\(outputAlive)")
        }
        if outputRates != previous.outputRates {
            changes.append("output-rates=\(previous.outputRates)->\(outputRates)")
        }
        if outputBufferFrames != previous.outputBufferFrames {
            changes.append("output-buffer-frames=\(previous.outputBufferFrames)->\(outputBufferFrames)")
        }
        if outputStreams != previous.outputStreams {
            changes.append("output-streams=\(previous.outputStreams)->\(outputStreams)")
        }
        return changes
    }
}

extension SpeakerAudioCapture {
    var missingDeviceCount: Int { 0 }
    func close() throws { try stop() }
}

enum SpeakerAudioTapError: LocalizedError {
    case operation(String, OSStatus)
    case noOutputDevice
    case reconfiguring
    case speakerDevicesMissing(Int)
    case outputDeviceListUnavailable
    case currentProcessUnavailable
    case invalidFormat
    case unsupportedAggregateInput(String)
    case invalidAggregateClock(String)
    case invalidBuffer
    case invalidTimestamp
    case allocation
    case capacityLimit
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
        case .reconfiguring: return "system-audio-reconfiguring"
        case .speakerDevicesMissing: return "speaker-device-missing"
        case .outputDeviceListUnavailable: return "system-audio-device-list"
        case .invalidFormat, .invalidBuffer, .unsupportedAggregateInput: return "system-audio-format"
        case .capacityLimit: return "system-audio-capacity"
        case .invalidTimestamp, .invalidAggregateClock: return "system-audio-clock"
        case .overflow: return "system-audio-overflow"
        case .startupTimeout: return "system-audio-start-timeout"
        case .noAudio: return "system-audio-no-audio"
        case .cleanup: return "system-audio-cleanup"
        default: return "system-audio"
        }
    }

    var errorDescription: String? {
        switch self {
        case .operation(_, kAudioDevicePermissionsError): return "システムオーディオ録音が許可されていません。システム設定の「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で許可し、Hearing を入れ直してください"
        case let .operation(operation, status): return "Core Audio operation=\(operation) status=\(status)"
        case .noOutputDevice: return "利用可能な音声出力デバイスがありません"
        case .reconfiguring: return "スピーカー取り込みを再構成しています"
        case let .speakerDevicesMissing(count): return "指定した出力デバイスのうち \(count) 台が未接続です"
        case .outputDeviceListUnavailable: return "出力デバイス一覧を取得できません"
        case .currentProcessUnavailable: return "音声除外対象のプロセスを取得できませんでした"
        case .invalidFormat: return "スピーカー音声の形式を取得できませんでした"
        case let .unsupportedAggregateInput(reason): return "集約デバイスの入力形式に対応できません: \(reason)"
        case let .invalidAggregateClock(reason): return "集約デバイスのクロック源が不正です: \(reason)"
        case .invalidBuffer: return "スピーカー音声のバッファ形式が変わりました"
        case .invalidTimestamp: return "スピーカー音声の取得時刻を取得できませんでした"
        case .allocation: return "スピーカー音声のバッファを確保できませんでした"
        case .capacityLimit: return "スピーカー音声の事前確保が 64 MiB の上限に収まりません"
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
    private let diagnostic: (String) -> Void
    private let onStage: (String, String) -> Void
    private let systemListener: any SpeakerSystemPropertyListening
    private let selectedUIDs: [String]
    private let changes: OpaquePointer
    private var observedSequence: UInt64 = 0
    private var observedDeviceListSequence: UInt64 = 0
    private var observedProcessListSequence: UInt64 = 0
    private var configuration: SpeakerConfigurationSnapshot?
    private var outputID = AudioObjectID(kAudioObjectUnknown)
    private var tapIDs: [AudioObjectID] = []
    private var deviceID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var ring: OpaquePointer?
    private var buffer: AVAudioPCMBuffer?
    private let ringLock = NSLock()
    private var listeners: [(AudioObjectID, AudioObjectPropertyAddress)] = []
    private var outputListenerInstalled = false
    private var deviceListListenerInstalled = false
    private var processListListenerInstalled = false
    private var deviceStarted = false
    private var cleanupFailed = false
    private var activityCheckAt = Date.distantPast
    private var metricsReportedAt = Date.distantPast
    private var missingAudioSince: Date?
    private var receivedCount: UInt64 = 0
    private(set) var missingDeviceCount = 0
    private var selectedOutputIDs: [AudioObjectID] = []
    private var tapCreateCount = 0
    private var aggregateCreateCount = 0
    private var startCount = 0
    private var stopCount = 0
    private var rebuildCount = 0

    private let exclusionLock = NSLock()
    private var excludedProcessPIDs: [pid_t]
    private var exclusionCommandSequence: UInt64 = 0
    private var observedExclusionCommandSequence: UInt64 = 0
    private let parentPID: pid_t?
    private var activeExclusions: SpeakerProcessExclusions?
    private var siblingCheckAt = Date.distantPast

    init(selectedUIDs: [String], excludedProcessPIDs: [pid_t] = [], parentPID: pid_t? = nil, diagnostic: @escaping (String) -> Void, onStage: @escaping (String, String) -> Void,
         systemListener: any SpeakerSystemPropertyListening = CoreAudioSystemPropertyListener()) throws {
        self.selectedUIDs = selectedUIDs
        self.excludedProcessPIDs = excludedProcessPIDs
        self.parentPID = parentPID
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
        address = Self.address(kAudioHardwarePropertyProcessObjectList)
        let status = systemListener.add(&address, changes: changes)
        if status == noErr {
            processListListenerInstalled = true
        } else {
            diagnostic("audio-tap-exclusion operation=observe-process-list status=\(status)")
        }
    }

    func updateExcludedProcessPIDs(_ pids: [pid_t]) {
        exclusionLock.lock()
        excludedProcessPIDs = pids
        exclusionCommandSequence &+= 1
        exclusionLock.unlock()
    }

    private func currentExclusionCommand() -> (pids: [pid_t], sequence: UInt64) {
        exclusionLock.lock()
        defer { exclusionLock.unlock() }
        return (excludedProcessPIDs, exclusionCommandSequence)
    }

    func start(onPrepared: (AVAudioFormat, SpeakerBufferCapacity) throws -> Void,
               onCapturing: () -> Void) throws -> AVAudioFormat {
        guard !cleanupFailed else { throw SpeakerAudioTapError.operation("previous-cleanup", kAudioHardwareUnspecifiedError) }
        if !selectedUIDs.isEmpty && (tapCreateCount > 0 || aggregateCreateCount > 0) {
            rebuildCount += 1
            reportSelectedLifecycle()
        }
        selectedOutputIDs.removeAll()
        observedSequence = coosenpai_audio_changes_sequence(changes)
        observedDeviceListSequence = coosenpai_audio_device_list_sequence(changes)
        observedProcessListSequence = coosenpai_audio_process_list_sequence(changes)
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
            let siblings = parentPID.flatMap { siblingHearingPIDs(parentPID: $0, diagnostic: diagnostic) } ?? []
            siblingCheckAt = Date()
            let command = currentExclusionCommand()
            observedExclusionCommandSequence = command.sequence
            let requested = speakerCurrentExclusionPIDs(
                configured: command.pids, parentPID: parentPID, siblings: siblings,
                isSibling: isCurrentSiblingHelper
            )
            let resolution = SpeakerProcessExclusions.resolve(
                requested: requested, currentPID: getpid(), currentObject: processID,
                processObject: processObject(for:)
            )
            activeExclusions = resolution
            let excluded = resolution.objects
            diagnostic("audio-tap-exclusion process-count=\(excluded.count) failed-pid-count=\(resolution.failedPIDs.count)")
            var tapUIDs: [String] = []
            let targets: [(uid: String, stream: UInt)?] = selectedUIDs.isEmpty ? [nil] : tapTargets.map(Optional.some)
            for target in targets {
                let description = target.map {
                    CATapDescription(excludingProcesses: excluded, deviceUID: $0.uid, stream: $0.stream)
                } ?? CATapDescription(stereoGlobalTapButExcludeProcesses: excluded)
                description.name = "CooSenpAI Hearing"
                description.isPrivate = true
                description.muteBehavior = .unmuted
                if target != nil {
                    description.isMixdown = true
                    description.isMono = true
                }
                var tapID = AudioObjectID(kAudioObjectUnknown)
                try check(AudioHardwareCreateProcessTap(description, &tapID), "create-tap")
                if !selectedUIDs.isEmpty { tapCreateCount += 1 }
                tapIDs.append(tapID)
                tapUIDs.append(try readStringProperty(tapID, kAudioTapPropertyUID, operation: "read-tap-uid"))
            }
            if !selectedUIDs.isEmpty { reportSelectedLifecycle() }
            onStage("tap-create", "end")
            onStage("aggregate-create", "begin")
            let plan = try Self.aggregatePlan(
                tapUIDs: tapUIDs, tapDeviceUIDs: tapTargets.map(\.uid), selected: !selectedUIDs.isEmpty
            )
            let deviceDescription = plan.description
            try check(AudioHardwareCreateAggregateDevice(deviceDescription as CFDictionary, &deviceID), "create-aggregate")
            if !selectedUIDs.isEmpty { aggregateCreateCount += 1; reportSelectedLifecycle() }
            onStage("aggregate-ready", "begin")
            try waitForDeviceAlive(deviceID)
            onStage("aggregate-ready", "end")
            if !plan.attachAfterCreation.isEmpty { try attachTaps(plan.attachAfterCreation, to: deviceID) }
            if tapIDs.count > 1, #available(macOS 15.0, *) {
                requestTapClockSource()
            }
            var streamDescription: AudioStreamBasicDescription
            var aggregateInputDescription: AudioStreamBasicDescription?
            var inputChannels: [UInt32] = []
            var tapChannels: [UInt32] = []
            if selectedUIDs.isEmpty {
                streamDescription = try readStreamDescription(tapIDs[0], kAudioTapPropertyFormat)
            } else {
                let tapFormats = try tapIDs.map { try readStreamDescription($0, kAudioTapPropertyFormat) }
                let aggregateFormat = try readStreamDescription(
                    deviceID, kAudioDevicePropertyStreamFormat, scope: kAudioDevicePropertyScopeInput
                )
                aggregateInputDescription = aggregateFormat
                inputChannels = try aggregateInputBufferChannels(deviceID)
                diagnostic("audio-tap-input buffers=\(inputChannels.count) channels=\(inputChannels)")
                let nominalRate = try doubleProperty(deviceID, kAudioDevicePropertyNominalSampleRate)
                tapChannels = try SpeakerAggregateInputLayout.tapChannels(
                    tapFormats: tapFormats, aggregateFormat: aggregateFormat,
                    bufferChannels: inputChannels, nominalRate: nominalRate
                )
                diagnostic("audio-tap-input taps=\(tapChannels.count) tap-channels=\(tapChannels)")
                streamDescription = aggregateFormat
                streamDescription.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked
                    | kAudioFormatFlagIsNonInterleaved
                streamDescription.mChannelsPerFrame = UInt32(tapChannels.count)
                streamDescription.mBytesPerFrame = 4
                streamDescription.mBytesPerPacket = 4
                streamDescription.mFramesPerPacket = 1
                streamDescription.mBitsPerChannel = 32
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
            let callbackFrames = try uint32Property(deviceID, kAudioDevicePropertyBufferFrameSize)
            guard let format, format.channelCount > 0 else {
                throw SpeakerAudioTapError.invalidFormat
            }
            let ringBufferCount = selectedUIDs.isEmpty
                ? (streamDescription.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
                    ? UInt64(streamDescription.mChannelsPerFrame) : 1)
                : 1
            let ringBytesPerFrame = try selectedUIDs.isEmpty
                ? UInt64(streamDescription.mBytesPerFrame)
                : SpeakerAggregateInputLayout.ringFrameBytes(bufferChannels: inputChannels)
            let outputDescription = format.streamDescription.pointee
            let outputBufferCount = outputDescription.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0
                ? UInt64(outputDescription.mChannelsPerFrame) : 1
            let capacities = try SpeakerBufferCapacity(
                sampleRate: format.sampleRate, callbackFrames: callbackFrames,
                ringBytesPerFrame: ringBytesPerFrame, ringBufferCount: ringBufferCount,
                outputBytesPerFrame: UInt64(outputDescription.mBytesPerFrame), outputBufferCount: outputBufferCount
            )
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: capacities.frameCapacity) else {
                throw SpeakerAudioTapError.allocation
            }
            let ring: OpaquePointer?
            if selectedUIDs.isEmpty {
                ring = coosenpai_audio_ring_create(&streamDescription, capacities.frameCapacity,
                                                  capacities.slotCount, changes)
            } else {
                guard var inputDescription = aggregateInputDescription else { throw SpeakerAudioTapError.invalidFormat }
                ring = inputChannels.withUnsafeBufferPointer { buffers in
                    tapChannels.withUnsafeBufferPointer { taps in
                        guard let bufferAddress = buffers.baseAddress, let tapAddress = taps.baseAddress else {
                            return nil
                        }
                        return coosenpai_audio_ring_create_tap_mix(
                            &inputDescription, bufferAddress, UInt32(buffers.count),
                            tapAddress, UInt32(taps.count), capacities.frameCapacity,
                            capacities.slotCount, changes
                        )
                    }
                }
            }
            guard let ring else {
                throw SpeakerAudioTapError.allocation
            }
            ringLock.lock()
            self.buffer = buffer
            self.ring = ring
            ringLock.unlock()
            onStage("aggregate-create", "end")
            onStage("add-output", "begin")
            try check(AudioDeviceCreateIOProcIDWithBlock(
                &ioProcID, deviceID, nil
            ) { _, inputData, inputTime, _, _ in
                let started = mach_absolute_time()
                let time = inputTime.pointee
                let hostTime = time.mFlags.contains(.hostTimeValid) ? time.mHostTime : 0
                coosenpai_audio_ring_push(ring, inputData, hostTime)
                coosenpai_audio_ring_record_callback_host_ticks(ring, mach_absolute_time() - started)
            }, "create-io-proc")
            onStage("add-output", "end")
            if tapIDs.count > 1 {
                if #available(macOS 15.0, *) {
                    try alignTapDriftCompensation(tapUIDs: tapUIDs, tapDeviceUIDs: tapTargets.map(\.uid))
                } else {
                    // MainSubDevice reports an AudioDevice UID, so it cannot identify the clock tap here.
                    diagnostic("audio-tap-clock verification=unavailable basis=configured-tap-0")
                }
            }
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
            configuration = try configurationSnapshot()
            missingAudioSince = nil
            activityCheckAt = .distantPast
            metricsReportedAt = .distantPast
            receivedCount = 0
            diagnostic("speaker-buffer-capacity sample-rate=\(Int(format.sampleRate)) "
                + "callback-frames=\(callbackFrames) slots=\(capacities.slotCount) "
                + "frame-capacity=\(capacities.frameCapacity) preallocated-bytes=\(capacities.preallocatedBytes)")
            try onPrepared(format, capacities)
            onStage("start-capture", "begin")
            onCapturing()
            try check(AudioDeviceStart(deviceID, ioProcID), "start-device")
            deviceStarted = true
            if !selectedUIDs.isEmpty { startCount += 1; reportSelectedLifecycle() }
            onStage("start-capture", "end")
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
            address(kAudioDevicePropertyStreamConfiguration, scope: kAudioDevicePropertyScopeInput),
            address(kAudioDevicePropertyDeviceIsAlive),
            address(kAudioDevicePropertyNominalSampleRate),
            address(kAudioDevicePropertyBufferFrameSize),
        ]
    }

    struct AggregatePlan {
        let description: [String: Any]
        let attachAfterCreation: [String]
    }

    static func aggregatePlan(tapUIDs: [String], tapDeviceUIDs: [String], selected: Bool) throws -> AggregatePlan {
        var description: [String: Any] = [
            kAudioAggregateDeviceNameKey: "CooSenpAI Hearing",
            kAudioAggregateDeviceUIDKey: SpeakerOutputDevices.aggregateUIDPrefix + UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            // Waiting for a tapped application can block AudioDeviceStart indefinitely.
            kAudioAggregateDeviceTapAutoStartKey: false,
        ]
        if selected {
            guard !tapUIDs.isEmpty, tapUIDs.count == tapDeviceUIDs.count else {
                throw SpeakerAudioTapError.unsupportedAggregateInput("tap UID と出力デバイスの対応が一致しません")
            }
            let flags = try driftFlags(tapDeviceUIDs: tapDeviceUIDs, clockIndex: 0)
            description[kAudioAggregateDeviceTapListKey] = zip(tapUIDs, flags).map { tapUID, drift in
                [
                    kAudioSubTapUIDKey: tapUID,
                    kAudioSubTapDriftCompensationKey: drift,
                ] as [String: Any]
            }
            return AggregatePlan(description: description, attachAfterCreation: [])
        }
        guard tapUIDs.count == 1, tapDeviceUIDs.isEmpty else {
            throw SpeakerAudioTapError.unsupportedAggregateInput("global tap の構成が不正です")
        }
        return AggregatePlan(description: description, attachAfterCreation: tapUIDs)
    }

    static func driftFlags(tapDeviceUIDs: [String], clockIndex: Int) throws -> [Bool] {
        guard tapDeviceUIDs.indices.contains(clockIndex) else {
            throw SpeakerAudioTapError.invalidAggregateClock("クロック源が選択した tap に含まれません")
        }
        let clockDeviceUID = tapDeviceUIDs[clockIndex]
        return tapDeviceUIDs.map { $0 != clockDeviceUID }
    }

    static func driftFlags(
        tapUIDs: [String], tapDeviceUIDs: [String], clockIndex: Int, subTapUIDs: [String]
    ) throws -> [Bool] {
        guard tapUIDs.count == tapDeviceUIDs.count,
              Set(tapUIDs).count == tapUIDs.count,
              subTapUIDs.count == tapUIDs.count,
              Set(subTapUIDs).count == subTapUIDs.count else {
            throw SpeakerAudioTapError.invalidAggregateClock("tap と sub-tap の UID が一致しません")
        }
        let flags = try driftFlags(tapDeviceUIDs: tapDeviceUIDs, clockIndex: clockIndex)
        let byUID = Dictionary(uniqueKeysWithValues: zip(tapUIDs, flags))
        return try subTapUIDs.map { uid in
            guard let flag = byUID[uid] else {
                throw SpeakerAudioTapError.invalidAggregateClock("不明な sub-tap UID です")
            }
            return flag
        }
    }

    @available(macOS 15.0, *)
    private func requestTapClockSource() {
        do {
            try AudioHardwareAggregateDevice(id: deviceID).setClockSource(AudioHardwareTap(id: tapIDs[0]))
            diagnostic("audio-tap-clock request=tap-0 result=success")
        } catch {
            diagnostic("audio-tap-clock request=tap-0 result=failed basis=configured-tap-0 error=\(error)")
        }
    }

    static func clockIndex(
        readClockID: () throws -> AudioObjectID?, tapIDs: [AudioObjectID], diagnostic: (String) -> Void
    ) throws -> Int? {
        let clockID: AudioObjectID
        do {
            guard let id = try readClockID() else {
                diagnostic("audio-tap-clock verification=unavailable reason=unset basis=configured-tap-0")
                return nil
            }
            clockID = id
        } catch {
            diagnostic("audio-tap-clock verification=unavailable reason=read-failed basis=configured-tap-0 error=\(error)")
            return nil
        }
        guard let index = tapIDs.firstIndex(of: clockID) else {
            throw SpeakerAudioTapError.invalidAggregateClock("クロック源が選択した tap に含まれません")
        }
        diagnostic("audio-tap-clock tap-index=\(index) tap-id=\(clockID)")
        return index
    }

    @available(macOS 15.0, *)
    private func alignTapDriftCompensation(tapUIDs: [String], tapDeviceUIDs: [String]) throws {
        guard tapIDs.count == tapDeviceUIDs.count else {
            throw SpeakerAudioTapError.invalidAggregateClock("tap と出力デバイスの数が一致しません")
        }
        guard let clockIndex = try Self.clockIndex(
            readClockID: { try AudioHardwareAggregateDevice(id: deviceID).clockSource?.id },
            tapIDs: tapIDs, diagnostic: diagnostic
        ) else { return }

        var tapAddress = Self.address(kAudioAggregateDevicePropertyTapList)
        var tapListSize: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(deviceID, &tapAddress, 0, nil, &tapListSize),
                  "read-aggregate-tap-list-size")
        guard tapListSize > 0 else {
            throw SpeakerAudioTapError.invalidAggregateClock("集約デバイスに tap がありません")
        }
        var tapList: CFArray?
        try check(withUnsafeMutablePointer(to: &tapList) { pointer in
            AudioObjectGetPropertyData(deviceID, &tapAddress, 0, nil, &tapListSize, pointer)
        }, "read-aggregate-tap-list")
        guard let actualTapUIDs = tapList as? [CFString],
              actualTapUIDs.map({ $0 as String }) == tapUIDs else {
            throw SpeakerAudioTapError.invalidAggregateClock("集約デバイスの tap 順序が異なります")
        }

        var address = Self.address(kAudioAggregateDevicePropertySubTapList)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(deviceID, &address, 0, nil, &size), "read-subtap-list-size")
        guard size == UInt32(tapIDs.count * MemoryLayout<AudioObjectID>.stride) else {
            throw SpeakerAudioTapError.invalidAggregateClock("sub-tap 数が tap 数と一致しません")
        }
        var subTapIDs = [AudioObjectID](repeating: AudioObjectID(kAudioObjectUnknown), count: tapIDs.count)
        try subTapIDs.withUnsafeMutableBytes { bytes in
            try check(AudioObjectGetPropertyData(deviceID, &address, 0, nil, &size, bytes.baseAddress!),
                      "read-subtap-list")
        }
        guard subTapIDs.allSatisfy({ $0 != kAudioObjectUnknown }) else {
            throw SpeakerAudioTapError.invalidAggregateClock("sub-tap ID が不正です")
        }
        let subTapUIDs = try subTapIDs.map { try AudioHardwareTap(id: $0).uid }
        let flags = try Self.driftFlags(
            tapUIDs: tapUIDs, tapDeviceUIDs: tapDeviceUIDs,
            clockIndex: clockIndex, subTapUIDs: subTapUIDs
        )
        for (subTapID, flag) in zip(subTapIDs, flags) {
            var driftAddress = Self.address(kAudioSubTapPropertyDriftCompensation)
            var drift: UInt32 = flag ? 1 : 0
            var current: UInt32 = 0
            var propertySize = UInt32(MemoryLayout<UInt32>.size)
            try check(AudioObjectGetPropertyData(subTapID, &driftAddress, 0, nil, &propertySize, &current),
                      "read-subtap-drift")
            guard propertySize == MemoryLayout<UInt32>.size else {
                throw SpeakerAudioTapError.invalidAggregateClock("sub-tap の補正値が不正です")
            }
            if current != drift {
                try check(AudioObjectSetPropertyData(subTapID, &driftAddress, 0, nil,
                                                     propertySize, &drift), "set-subtap-drift")
            }
        }
    }

    private func reportSelectedLifecycle() {
        emitSpeakerLoadDiagnostic(
            "audio-tap-selected tap-create=\(tapCreateCount) aggregate-create=\(aggregateCreateCount) "
                + "start=\(startCount) stop=\(stopCount) rebuild=\(rebuildCount)",
            via: diagnostic
        )
    }

    private func processObject(for pid: pid_t) -> AudioObjectID? {
        var address = Self.address(kAudioHardwarePropertyTranslatePIDToProcessObject)
        var candidatePID = pid
        var object = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        let status = AudioObjectGetPropertyData(
            AudioObjectID(kAudioObjectSystemObject), &address,
            UInt32(MemoryLayout<pid_t>.size), &candidatePID, &size, &object
        )
        guard status == noErr, object != kAudioObjectUnknown else {
            diagnostic("audio-tap-exclusion pid=\(pid) unavailable status=\(status)")
            return nil
        }
        return object
    }

    func takeConfigurationChange() throws -> String? {
        let processListSequence = coosenpai_audio_process_list_sequence(changes)
        let processListChanged = processListSequence != observedProcessListSequence
        observedProcessListSequence = processListSequence
        let command = currentExclusionCommand()
        let commandChanged = command.sequence != observedExclusionCommandSequence
        observedExclusionCommandSequence = command.sequence
        if let parentPID,
           commandChanged || processListChanged || Date().timeIntervalSince(siblingCheckAt) >= 1 {
            siblingCheckAt = Date()
            let siblings = siblingHearingPIDs(parentPID: parentPID, diagnostic: diagnostic) ?? []
            let current = speakerCurrentExclusionPIDs(
                configured: command.pids, parentPID: parentPID, siblings: siblings,
                isSibling: isCurrentSiblingHelper
            )
            if let activeExclusions {
                let resolved = SpeakerProcessExclusions.resolve(
                    requested: current, currentPID: getpid(), currentObject: activeExclusions.objects[0],
                    processObject: processObject(for:)
                )
                if activeExclusions.needsRebuild(resolved: resolved) {
                    return "session-helper-processes-changed"
                }
                if activeExclusions.failedPIDs != resolved.failedPIDs {
                    diagnostic("audio-tap-exclusion process-count=\(resolved.objects.count) failed-pid-count=\(resolved.failedPIDs.count)")
                }
                self.activeExclusions = resolved
            }
        }
        let sequence = coosenpai_audio_changes_sequence(changes)
        let listSequence = coosenpai_audio_device_list_sequence(changes)
        let propertyChanged = sequence != observedSequence
        let listChanged = listSequence != observedDeviceListSequence
        guard propertyChanged || listChanged else { return nil }
        observedSequence = sequence
        observedDeviceListSequence = listSequence
        guard deviceID != kAudioObjectUnknown, configuration != nil else {
            return "capture-retry=1"
        }
        if !propertyChanged && selectedUIDs.isEmpty { return nil }
        if selectedUIDs.isEmpty {
            let currentOutput = try uint32Property(AudioObjectID(kAudioObjectSystemObject),
                                                   kAudioHardwarePropertyDefaultOutputDevice)
            if currentOutput != outputID {
                return "output-ids=[\(outputID)]->[\(currentOutput)]"
            }
        }
        let current: SpeakerConfigurationSnapshot
        do { current = try configurationSnapshot() }
        catch SpeakerAudioTapError.outputDeviceListUnavailable {
            return "output-device-list-unavailable"
        }
        catch SpeakerAudioTapError.operation(_, kAudioHardwareBadObjectError) {
            return "device-alive=1->0"
        }
        catch SpeakerAudioTapError.operation(_, kAudioHardwareBadDeviceError) {
            return "device-alive=1->0"
        }
        guard let configuration else { throw SpeakerAudioTapError.invalidFormat }
        let reasons = current.changes(from: configuration)
        if reasons.isEmpty {
            if let ring { coosenpai_audio_ring_accept_configuration(ring, sequence) }
            return nil
        }
        return reasons.joined(separator: " ")
    }

    private func configurationSnapshot() throws -> SpeakerConfigurationSnapshot {
        let aggregateAlive = try deviceAlive(deviceID)
        let inputFormats = try aggregateAlive == 0 ? [] :
            (tapIDs.map { try streamFormat($0, kAudioTapPropertyFormat) }
             + [streamFormat(deviceID, kAudioDevicePropertyStreamFormat, scope: kAudioDevicePropertyScopeInput)])
        let aggregateInputChannels = try aggregateAlive == 0 || selectedUIDs.isEmpty
            ? [] : aggregateInputBufferChannels(deviceID)
        let outputDevices: [SpeakerOutputDevice]
        if selectedUIDs.isEmpty {
            outputDevices = []
        } else {
            do { outputDevices = SpeakerOutputDevices.resolve(selectedUIDs, from: try SpeakerOutputDevices.available()).devices }
            catch { throw SpeakerAudioTapError.outputDeviceListUnavailable }
        }
        let outputIDs: [AudioObjectID]
        if selectedUIDs.isEmpty {
            let current = try uint32Property(AudioObjectID(kAudioObjectSystemObject), kAudioHardwarePropertyDefaultOutputDevice)
            outputIDs = current == kAudioObjectUnknown ? [] : [current]
        } else {
            outputIDs = outputDevices.map(\.objectID)
        }
        let outputAlive = try outputIDs.map { try deviceAlive($0) }
        let outputRates = try zip(outputIDs, outputAlive).map { id, alive in
            alive == 0 ? 0 : try doubleProperty(id, kAudioDevicePropertyNominalSampleRate)
        }
        let outputBufferFrames = try zip(outputIDs, outputAlive).map { id, alive in
            alive == 0 ? 0 : try uint32Property(id, kAudioDevicePropertyBufferFrameSize)
        }
        return SpeakerConfigurationSnapshot(
            inputFormats: inputFormats,
            aggregateInputChannels: aggregateInputChannels,
            aggregateRate: aggregateAlive == 0 ? 0 : try doubleProperty(deviceID, kAudioDevicePropertyNominalSampleRate),
            aggregateBufferFrames: aggregateAlive == 0 ? 0 : try uint32Property(deviceID, kAudioDevicePropertyBufferFrameSize),
            aggregateAlive: aggregateAlive,
            outputIDs: outputIDs,
            outputAlive: outputAlive,
            outputRates: outputRates,
            outputBufferFrames: outputBufferFrames,
            outputStreams: outputDevices.map(\.outputStreams)
        )
    }

    private func readStreamDescription(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                                       scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> AudioStreamBasicDescription {
        var address = Self.address(selector, scope: scope)
        var format = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        try check(AudioObjectGetPropertyData(object, &address, 0, nil, &size, &format), "read-stream-format")
        guard size == MemoryLayout<AudioStreamBasicDescription>.size else { throw SpeakerAudioTapError.invalidFormat }
        return format
    }

    private func streamFormat(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                              scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> String {
        let format = try readStreamDescription(object, selector, scope: scope)
        return "\(format.mSampleRate),\(format.mFormatID),\(format.mFormatFlags),\(format.mBytesPerPacket),\(format.mFramesPerPacket),\(format.mBytesPerFrame),\(format.mChannelsPerFrame),\(format.mBitsPerChannel)"
    }

    private func deviceAlive(_ object: AudioObjectID) throws -> UInt32 {
        do { return try uint32Property(object, kAudioDevicePropertyDeviceIsAlive) }
        catch SpeakerAudioTapError.operation(_, kAudioHardwareBadObjectError) { return 0 }
        catch SpeakerAudioTapError.operation(_, kAudioHardwareBadDeviceError) { return 0 }
    }

    func nextBuffer(checkDelivery: Bool) throws -> SpeakerCapturedBuffer? {
        ringLock.lock()
        defer { ringLock.unlock() }
        guard let ring, let buffer else { return nil }
        reportRingMetrics(ring, force: false)
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
        if checkDelivery { try checkAudioDelivery(ring) }
        return nil
    }

    private func reportRingMetrics(_ ring: OpaquePointer, force: Bool) {
        let now = Date()
        guard force || now.timeIntervalSince(metricsReportedAt) >= 5 else { return }
        metricsReportedAt = now
        let callbackNanoseconds = AudioConvertHostTimeToNanos(coosenpai_audio_ring_max_callback_host_ticks(ring))
        let message = "audio-tap-load received=\(coosenpai_audio_ring_received(ring)) "
            + "overflows=\(coosenpai_audio_ring_overflows(ring)) "
            + "max-ring-depth=\(coosenpai_audio_ring_max_depth(ring)) "
            + "max-io-proc-us=\(callbackNanoseconds / 1_000)"
        emitSpeakerLoadDiagnostic(message, via: diagnostic)
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
        ringLock.lock()
        if let ring { reportRingMetrics(ring, force: true) }
        ringLock.unlock()
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
                if Self.cleanupReleased(status) {
                    deviceStarted = false
                    if !selectedUIDs.isEmpty { stopCount += 1; reportSelectedLifecycle() }
                }
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
            ringLock.lock()
            if let ring {
                coosenpai_audio_ring_destroy(ring)
                self.ring = nil
            }
            buffer = nil
            ringLock.unlock()
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
        configuration = nil
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
        if processListListenerInstalled {
            var address = Self.address(kAudioHardwarePropertyProcessObjectList)
            let status = systemListener.remove(&address, changes: changes)
            if Self.cleanupReleased(status) {
                processListListenerInstalled = false
            } else {
                diagnostic("audio-tap-cleanup operation=remove-process-list-observer status=\(status)")
                failures.append("operation=remove-process-list-observer status=\(status)")
            }
        }
        if !failures.isEmpty {
            throw SpeakerAudioTapError.cleanup(failures)
        }
    }

    deinit {
        do { try close() }
        catch { diagnostic("audio-tap-cleanup operation=deinit detail=\(error.localizedDescription)") }
        if !outputListenerInstalled && !deviceListListenerInstalled && !processListListenerInstalled
            && listeners.isEmpty && ring == nil {
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

    private func aggregateInputBufferChannels(_ aggregate: AudioObjectID) throws -> [UInt32] {
        var address = Self.address(kAudioDevicePropertyStreamConfiguration, scope: kAudioDevicePropertyScopeInput)
        var size: UInt32 = 0
        try check(AudioObjectGetPropertyDataSize(aggregate, &address, 0, nil, &size), "read-aggregate-channels-size")
        guard size >= MemoryLayout<AudioBufferList>.size else { throw SpeakerAudioTapError.invalidFormat }
        let storage = UnsafeMutableRawPointer.allocate(byteCount: Int(size), alignment: MemoryLayout<AudioBufferList>.alignment)
        defer { storage.deallocate() }
        let list = storage.bindMemory(to: AudioBufferList.self, capacity: 1)
        try check(AudioObjectGetPropertyData(aggregate, &address, 0, nil, &size, list), "read-aggregate-channels")
        guard list.pointee.mNumberBuffers > 0,
              Int(size) >= MemoryLayout<AudioBufferList>.size
                + (Int(list.pointee.mNumberBuffers) - 1) * MemoryLayout<AudioBuffer>.stride else {
            throw SpeakerAudioTapError.invalidFormat
        }
        let buffers = UnsafeMutableAudioBufferListPointer(list)
        return buffers.map(\.mNumberChannels)
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
