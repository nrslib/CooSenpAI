import Foundation

enum RecognitionRestartThrottle {
    static let minimumIntervalNanoseconds: UInt64 = 2_000_000_000
}

enum RecognitionSegmentLifecycle: Equatable {
    case accepting
    case ending
    case terminal
    case cancelling
}

final class RecognitionCallbackGate {
    private let lock = NSLock()
    private var isOpen = false
    private var isDraining = false
    private var isDiscarded = false
    private var pendingCallbacks: [() -> Void] = []

    func enqueue(_ callback: @escaping () -> Void) {
        lock.lock()
        guard !isDiscarded else {
            lock.unlock()
            return
        }
        guard isOpen, !isDraining else {
            pendingCallbacks.append(callback)
            lock.unlock()
            return
        }
        lock.unlock()
        callback()
    }

    func open() {
        lock.lock()
        guard !isDiscarded else {
            lock.unlock()
            return
        }
        isOpen = true
        isDraining = true
        lock.unlock()
        while true {
            lock.lock()
            guard !isDiscarded else {
                lock.unlock()
                return
            }
            guard !pendingCallbacks.isEmpty else {
                isDraining = false
                lock.unlock()
                return
            }
            let callbacks = pendingCallbacks
            pendingCallbacks.removeAll(keepingCapacity: false)
            lock.unlock()
            callbacks.forEach { $0() }
        }
    }

    func discard() {
        lock.lock()
        isDiscarded = true
        pendingCallbacks.removeAll(keepingCapacity: false)
        lock.unlock()
    }
}

struct RecognitionRestartTracker {
    static let windowNanoseconds: UInt64 = 60_000_000_000
    static let maximumRestartCount = 30

    private(set) var restartTimes: [UInt64] = []

    mutating func recordRestart(at now: UInt64) -> Bool {
        if now >= Self.windowNanoseconds {
            let cutoff = now - Self.windowNanoseconds
            restartTimes.removeAll { $0 <= cutoff }
        }
        restartTimes.append(now)
        return restartTimes.count > Self.maximumRestartCount
    }

    var recentRestartCount: Int { restartTimes.count }
}

struct AudioSourceAvailability {
    private(set) var activeSources: Set<AudioSource>

    init(sources: Set<AudioSource>) {
        activeSources = sources
    }

    var hasActiveSource: Bool { !activeSources.isEmpty }

    func isActive(_ source: AudioSource) -> Bool {
        activeSources.contains(source)
    }

    mutating func disable(_ source: AudioSource) -> Bool {
        activeSources.remove(source) != nil
    }
}

struct RecognitionState<Request, Task, Recognizer> {
    let source: AudioSource
    let request: Request
    let task: Task
    let recognizer: Recognizer
    let generation: Int
    var lifecycle: RecognitionSegmentLifecycle
    var taskTerminalArrived: Bool
    var taskCancellationRequested: Bool
    var closeReason: RecognitionSegmentCloseReason?
}

struct RecognitionStateStore<Request, Task, Recognizer> {
    typealias State = RecognitionState<Request, Task, Recognizer>

    private struct GenerationKey: Hashable {
        let source: AudioSource
        let generation: Int
    }

    private var states: [AudioSource: State] = [:]
    private var latestGenerations: [AudioSource: Int] = [:]
    private var terminalGenerations: Set<GenerationKey> = []
    private var retiredGenerations: Set<GenerationKey> = []

    mutating func reserveGeneration(for source: AudioSource) -> Int {
        let generation = (latestGenerations[source] ?? 0) + 1
        latestGenerations[source] = generation
        retiredGenerations = retiredGenerations.filter { $0.source != source || $0.generation >= generation }
        return generation
    }

    mutating func install(
        source: AudioSource,
        request: Request,
        task: Task,
        recognizer: Recognizer,
        generation: Int,
        sourceIsActive: Bool
    ) -> Bool {
        let key = GenerationKey(source: source, generation: generation)
        guard sourceIsActive,
              latestGenerations[source] == generation,
              states[source] == nil,
              !retiredGenerations.contains(key) else {
            return false
        }
        let taskTerminalArrived = terminalGenerations.remove(key) != nil
        states[source] = State(
            source: source,
            request: request,
            task: task,
            recognizer: recognizer,
            generation: generation,
            lifecycle: taskTerminalArrived ? .terminal : .accepting,
            taskTerminalArrived: taskTerminalArrived,
            taskCancellationRequested: false,
            closeReason: nil
        )
        return true
    }

    mutating func markTaskTerminal(source: AudioSource, generation: Int) -> Bool {
        guard latestGenerations[source] == generation else { return false }
        let key = GenerationKey(source: source, generation: generation)
        guard !retiredGenerations.contains(key) else { return false }
        guard var state = states[source] else {
            terminalGenerations.insert(
                key
            )
            return true
        }
        guard state.generation == generation,
              !state.taskTerminalArrived else {
            return false
        }
        state.taskTerminalArrived = true
        if state.lifecycle == .accepting {
            state.lifecycle = .terminal
        }
        states[source] = state
        return true
    }

    mutating func beginEnding(
        source: AudioSource,
        generation: Int,
        reason: RecognitionSegmentCloseReason? = nil
    ) -> State? {
        guard var state = states[source],
              state.generation == generation,
              state.lifecycle == .accepting else {
            return nil
        }
        state.lifecycle = .ending
        state.closeReason = reason
        states[source] = state
        return state
    }

    mutating func beginCancelling(source: AudioSource, generation: Int) -> State? {
        guard var state = states[source],
              state.generation == generation,
              state.lifecycle == .ending,
              !state.taskTerminalArrived else {
            return nil
        }
        state.lifecycle = .cancelling
        state.taskCancellationRequested = true
        states[source] = state
        return state
    }

    mutating func recoverCancellationTimeout(
        source: AudioSource,
        generation: Int
    ) -> State? {
        guard let state = states[source],
              state.generation == generation,
              state.lifecycle == .cancelling,
              !state.taskTerminalArrived else {
            return nil
        }
        return remove(source: source, generation: generation)
    }

    func lifecycle(for source: AudioSource) -> RecognitionSegmentLifecycle? {
        states[source]?.lifecycle
    }

    func acceptsAudio(for source: AudioSource) -> Bool {
        states[source]?.lifecycle == .accepting
    }

    func isCurrentGeneration(_ source: AudioSource, _ generation: Int) -> Bool {
        latestGenerations[source] == generation
    }

    func isCurrentState(_ source: AudioSource, _ generation: Int) -> Bool {
        states[source]?.generation == generation
            && latestGenerations[source] == generation
    }

    func currentRequest(for source: AudioSource) -> Request? {
        states[source]?.request
    }

    func currentGeneration(for source: AudioSource) -> Int? {
        states[source]?.generation
    }

    func taskTerminalArrived(for source: AudioSource) -> Bool {
        states[source]?.taskTerminalArrived == true
    }

    func taskCancellationRequested(for source: AudioSource) -> Bool {
        states[source]?.taskCancellationRequested == true
    }

    func closeReason(for source: AudioSource) -> RecognitionSegmentCloseReason? {
        states[source]?.closeReason
    }

    mutating func remove(source: AudioSource, generation: Int) -> State? {
        let key = GenerationKey(source: source, generation: generation)
        guard states[source]?.generation == generation else {
            return nil
        }
        terminalGenerations.remove(
            key
        )
        retiredGenerations.insert(key)
        return states.removeValue(forKey: source)
    }

    mutating func retireGeneration(source: AudioSource, generation: Int) {
        guard latestGenerations[source] == generation else { return }
        let key = GenerationKey(source: source, generation: generation)
        terminalGenerations.remove(key)
        retiredGenerations.insert(key)
        if states[source]?.generation == generation {
            states.removeValue(forKey: source)
        }
    }

    mutating func removeAll() -> [State] {
        let currentStates = Array(states.values)
        retiredGenerations.formUnion(
            currentStates.map {
                GenerationKey(source: $0.source, generation: $0.generation)
            }
        )
        retiredGenerations.formUnion(
            latestGenerations.compactMap { source, generation in
                states[source] == nil
                    ? GenerationKey(source: source, generation: generation)
                    : nil
            }
        )
        self.states.removeAll()
        terminalGenerations.removeAll()
        return currentStates
    }
}
