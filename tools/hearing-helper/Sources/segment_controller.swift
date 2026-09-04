import AVFoundation
import Foundation

let recognitionFinalTimeoutNanoseconds: UInt64 = 4_000_000_000
let recognitionCancellationTimeoutNanoseconds: UInt64 = 1_000_000_000
let recognitionCancellationGraceNanoseconds: UInt64 = 4_000_000_000
let pendingAudioWindowCapacityNanoseconds = recognitionFinalTimeoutNanoseconds
    + RecognitionRestartThrottle.minimumIntervalNanoseconds
    + recognitionCancellationTimeoutNanoseconds
    + 1_000_000_000
let preRollAudioWindowCapacityNanoseconds = VoiceActivityConfiguration.standard.preRollNanoseconds

struct RecognitionCancellationDeadline: Equatable {
    let deadlineNanoseconds: UInt64

    init(startedAt: UInt64, timeoutNanoseconds: UInt64) {
        let (deadline, overflow) = startedAt.addingReportingOverflow(timeoutNanoseconds)
        precondition(!overflow, "recognition cancellation deadline overflowed")
        deadlineNanoseconds = deadline
    }

    func isExpired(at timestamp: UInt64) -> Bool {
        timestamp >= deadlineNanoseconds
    }

    func delayNanoseconds(from timestamp: UInt64) -> Int {
        guard deadlineNanoseconds > timestamp else { return 0 }
        return Int(min(deadlineNanoseconds - timestamp, UInt64(Int.max)))
    }
}

struct RecognitionCancellationTimeoutRecoveryTracker {
    static let maximumConsecutiveRecoveries = 3

    private(set) var consecutiveRecoveries = 0

    mutating func recordRecovery() -> Bool {
        consecutiveRecoveries += 1
        return consecutiveRecoveries >= Self.maximumConsecutiveRecoveries
    }

    mutating func reset() {
        consecutiveRecoveries = 0
    }
}

enum RecognitionSegmentCloseReason: String {
    case trailing
    case max
    case forced
    case steadyNoise = "steady-noise"
    case recognizerFinal = "recognizer-final"
    case noSpeech
    case error
}

enum RecognitionTaskCancelReason: String {
    case finalTimeout = "final-timeout"
    case sourceDisabled = "source-disabled"
    case sessionClosed = "session-closed"
    case registrationRejected = "registration-rejected"
}

enum RecognitionTaskOutcome {
    case success(text: String)
    case noSpeech
    case error(Error)
    case cancelled
}

func shouldRearmRecognitionImmediately(
    lifecycle: RecognitionSegmentLifecycle,
    vadWasSpeaking: Bool,
    closeReason: RecognitionSegmentCloseReason?
) -> Bool {
    vadWasSpeaking
        || lifecycle == .accepting
        || lifecycle == .terminal
        || closeReason == .trailing
}

enum PendingRecognitionAction: Equatable {
    case none
    case drainNow
    case drainAfter(UInt64)
}

struct RecognitionRestartDecision: Equatable {
    let recentCount: Int
    let thresholdReached: Bool
}

struct PendingAudioBuffer {
    let buffer: AVAudioPCMBuffer
    let rms: Double
    let timestamp: UInt64
}

enum PendingAudioAppendResult: Equatable {
    case appended
    case droppedOldest(count: Int)
    case rejected
}

struct RecognitionPendingCoordinator {
    private(set) var blockedUntil: UInt64?

    init() {
        blockedUntil = nil
    }

    var hasPendingCooldown: Bool { blockedUntil != nil }

    mutating func finishSegment(
        hasPendingAudio: Bool,
        cooldownNanoseconds: UInt64,
        at now: UInt64
    ) -> PendingRecognitionAction {
        guard cooldownNanoseconds > 0 else {
            blockedUntil = nil
            return hasPendingAudio ? .drainNow : .none
        }
        let (deadline, overflow) = now.addingReportingOverflow(cooldownNanoseconds)
        precondition(!overflow, "recognition cooldown deadline overflowed")
        blockedUntil = deadline
        return .drainAfter(deadline)
    }

    func isBlocked(at now: UInt64) -> Bool {
        guard let blockedUntil else { return false }
        return now < blockedUntil
    }

    mutating func markReady() {
        blockedUntil = nil
    }
}

struct RollingAudioWindow<Element> {
    private struct Entry {
        let value: Element
        let durationNanoseconds: UInt64
    }

    private let capacityNanoseconds: UInt64
    private var entries: [Entry] = []
    private var totalDurationNanoseconds: UInt64 = 0

    init(capacityNanoseconds: UInt64) {
        precondition(capacityNanoseconds > 0)
        self.capacityNanoseconds = capacityNanoseconds
    }

    var isEmpty: Bool { entries.isEmpty }

    mutating func append(_ value: Element, durationNanoseconds: UInt64) {
        precondition(durationNanoseconds > 0)
        let (newTotal, overflow) = totalDurationNanoseconds.addingReportingOverflow(
            durationNanoseconds
        )
        precondition(!overflow, "rolling audio duration overflowed")
        entries.append(Entry(value: value, durationNanoseconds: durationNanoseconds))
        totalDurationNanoseconds = newTotal
        trimToCapacity()
    }

    mutating func removeAll() -> [Element] {
        let values = entries.map(\.value)
        entries.removeAll(keepingCapacity: true)
        totalDurationNanoseconds = 0
        return values
    }

    private mutating func trimToCapacity() {
        while totalDurationNanoseconds > capacityNanoseconds {
            guard let first = entries.first else {
                preconditionFailure("rolling audio entries are missing")
            }
            entries.removeFirst()
            totalDurationNanoseconds -= first.durationNanoseconds
        }
    }
}

struct PendingAudioWindow<Element> {
    private struct Entry {
        let value: Element
        let durationNanoseconds: UInt64
    }

    private let capacityNanoseconds: UInt64
    private var entries: [Entry] = []
    private var totalDurationNanoseconds: UInt64 = 0

    init(capacityNanoseconds: UInt64) {
        precondition(capacityNanoseconds > 0)
        self.capacityNanoseconds = capacityNanoseconds
    }

    var isEmpty: Bool { entries.isEmpty }

    mutating func append(_ value: Element, durationNanoseconds: UInt64) -> Bool {
        let (newTotal, overflow) = totalDurationNanoseconds.addingReportingOverflow(
            durationNanoseconds
        )
        guard !overflow, newTotal <= capacityNanoseconds else { return false }
        entries.append(Entry(value: value, durationNanoseconds: durationNanoseconds))
        totalDurationNanoseconds = newTotal
        return true
    }

    mutating func appendKeepingLatest(
        _ value: Element,
        durationNanoseconds: UInt64
    ) -> PendingAudioAppendResult {
        guard durationNanoseconds > 0, durationNanoseconds <= capacityNanoseconds else {
            return .rejected
        }
        let (newTotal, overflow) = totalDurationNanoseconds.addingReportingOverflow(
            durationNanoseconds
        )
        guard !overflow else { return .rejected }
        entries.append(Entry(value: value, durationNanoseconds: durationNanoseconds))
        totalDurationNanoseconds = newTotal
        var droppedCount = 0
        while totalDurationNanoseconds > capacityNanoseconds {
            guard let first = entries.first else {
                preconditionFailure("pending audio entries are missing")
            }
            entries.removeFirst()
            totalDurationNanoseconds -= first.durationNanoseconds
            droppedCount += 1
        }
        return droppedCount == 0 ? .appended : .droppedOldest(count: droppedCount)
    }

    mutating func removeAll() -> [Element] {
        let pending = entries.map(\.value)
        entries.removeAll(keepingCapacity: true)
        totalDurationNanoseconds = 0
        return pending
    }
}

struct RecognitionSegmentController<Request, Task, Recognizer> {
    typealias State = RecognitionState<Request, Task, Recognizer>

    private var states = RecognitionStateStore<Request, Task, Recognizer>()
    private var pendingRecognition: [AudioSource: RecognitionPendingCoordinator]
    private var pendingAudioBuffers: [AudioSource: PendingAudioWindow<PendingAudioBuffer>]
    private var preRollAudioBuffers: [AudioSource: RollingAudioWindow<PendingAudioBuffer>]

    init(
        pendingCapacityNanoseconds: UInt64,
        preRollCapacityNanoseconds: UInt64
    ) {
        var pendingRecognition: [AudioSource: RecognitionPendingCoordinator] = [:]
        var pendingAudioBuffers: [AudioSource: PendingAudioWindow<PendingAudioBuffer>] = [:]
        var preRollAudioBuffers: [AudioSource: RollingAudioWindow<PendingAudioBuffer>] = [:]
        for source in AudioSource.allCases {
            pendingRecognition[source] = RecognitionPendingCoordinator()
            pendingAudioBuffers[source] = PendingAudioWindow(
                capacityNanoseconds: pendingCapacityNanoseconds
            )
            preRollAudioBuffers[source] = RollingAudioWindow(
                capacityNanoseconds: preRollCapacityNanoseconds
            )
        }
        self.pendingRecognition = pendingRecognition
        self.pendingAudioBuffers = pendingAudioBuffers
        self.preRollAudioBuffers = preRollAudioBuffers
    }

    mutating func reserveGeneration(for source: AudioSource) -> Int {
        states.reserveGeneration(for: source)
    }

    mutating func install(
        source: AudioSource,
        request: Request,
        task: Task,
        recognizer: Recognizer,
        generation: Int,
        sourceIsActive: Bool
    ) -> Bool {
        states.install(
            source: source,
            request: request,
            task: task,
            recognizer: recognizer,
            generation: generation,
            sourceIsActive: sourceIsActive
        )
    }

    mutating func markTaskTerminal(source: AudioSource, generation: Int) -> Bool {
        states.markTaskTerminal(source: source, generation: generation)
    }

    mutating func beginEnding(
        source: AudioSource,
        generation: Int,
        reason: RecognitionSegmentCloseReason? = nil
    ) -> State? {
        states.beginEnding(source: source, generation: generation, reason: reason)
    }

    mutating func beginCancelling(source: AudioSource, generation: Int) -> State? {
        states.beginCancelling(source: source, generation: generation)
    }

    mutating func recoverCancellationTimeout(
        source: AudioSource,
        generation: Int
    ) -> State? {
        states.recoverCancellationTimeout(source: source, generation: generation)
    }

    func lifecycle(for source: AudioSource) -> RecognitionSegmentLifecycle? {
        states.lifecycle(for: source)
    }

    func acceptsAudio(for source: AudioSource) -> Bool {
        states.acceptsAudio(for: source)
    }

    func isCurrentGeneration(_ source: AudioSource, _ generation: Int) -> Bool {
        states.isCurrentGeneration(source, generation)
    }

    func isCurrentState(_ source: AudioSource, _ generation: Int) -> Bool {
        states.isCurrentState(source, generation)
    }

    func currentRequest(for source: AudioSource) -> Request? {
        states.currentRequest(for: source)
    }

    func currentGeneration(for source: AudioSource) -> Int? {
        states.currentGeneration(for: source)
    }

    func taskTerminalArrived(for source: AudioSource) -> Bool {
        states.taskTerminalArrived(for: source)
    }

    func taskCancellationRequested(for source: AudioSource) -> Bool {
        states.taskCancellationRequested(for: source)
    }

    func closeReason(for source: AudioSource) -> RecognitionSegmentCloseReason? {
        states.closeReason(for: source)
    }

    mutating func remove(source: AudioSource, generation: Int) -> State? {
        states.remove(source: source, generation: generation)
    }

    mutating func retireGeneration(source: AudioSource, generation: Int) {
        states.retireGeneration(source: source, generation: generation)
    }

    mutating func removeAll() -> [State] {
        states.removeAll()
    }

    func hasPendingAudio(for source: AudioSource) -> Bool {
        pendingAudioBuffers[source]?.isEmpty == false
    }

    func hasPendingCooldown(for source: AudioSource) -> Bool {
        pendingRecognition[source]?.hasPendingCooldown == true
    }

    func isPendingBlocked(for source: AudioSource, at now: UInt64) -> Bool {
        pendingRecognition[source]?.isBlocked(at: now) == true
    }

    mutating func finishPendingSegment(
        for source: AudioSource,
        cooldownNanoseconds: UInt64,
        at now: UInt64
    ) -> PendingRecognitionAction {
        guard var coordinator = pendingRecognition[source] else {
            preconditionFailure("pending recognition state is missing for \(source.rawValue)")
        }
        let action = coordinator.finishSegment(
            hasPendingAudio: hasPendingAudio(for: source),
            cooldownNanoseconds: cooldownNanoseconds,
            at: now
        )
        pendingRecognition[source] = coordinator
        return action
    }

    mutating func markPendingReady(for source: AudioSource) {
        guard var coordinator = pendingRecognition[source] else {
            preconditionFailure("pending recognition state is missing for \(source.rawValue)")
        }
        coordinator.markReady()
        pendingRecognition[source] = coordinator
    }

    mutating func appendPending(
        _ value: PendingAudioBuffer,
        for source: AudioSource,
        durationNanoseconds: UInt64
    ) -> PendingAudioAppendResult {
        guard var pending = pendingAudioBuffers[source] else {
            preconditionFailure("pending audio buffer is missing for \(source.rawValue)")
        }
        let result = pending.appendKeepingLatest(
            value,
            durationNanoseconds: durationNanoseconds
        )
        pendingAudioBuffers[source] = pending
        return result
    }

    mutating func takePending(for source: AudioSource) -> [PendingAudioBuffer] {
        guard var pending = pendingAudioBuffers[source] else {
            preconditionFailure("pending audio buffer is missing for \(source.rawValue)")
        }
        let values = pending.removeAll()
        pendingAudioBuffers[source] = pending
        return values
    }

    mutating func appendPreRoll(
        _ value: PendingAudioBuffer,
        for source: AudioSource,
        durationNanoseconds: UInt64
    ) {
        guard var preRoll = preRollAudioBuffers[source] else {
            preconditionFailure("pre-roll audio buffer is missing for \(source.rawValue)")
        }
        preRoll.append(value, durationNanoseconds: durationNanoseconds)
        preRollAudioBuffers[source] = preRoll
    }

    mutating func takePreRoll(for source: AudioSource) -> [PendingAudioBuffer] {
        guard var preRoll = preRollAudioBuffers[source] else {
            preconditionFailure("pre-roll audio buffer is missing for \(source.rawValue)")
        }
        let values = preRoll.removeAll()
        preRollAudioBuffers[source] = preRoll
        return values
    }

    mutating func clearPendingAndPreRoll(for source: AudioSource) {
        _ = takePending(for: source)
        _ = takePreRoll(for: source)
    }
}
