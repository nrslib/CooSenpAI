import Foundation

func boundedPartialTranscript(_ text: String) -> String {
    var byteCount = 0
    var kept = String.UnicodeScalarView()
    for scalar in text.unicodeScalars {
        let size = String(scalar).utf8.count
        guard byteCount + size <= 4096 else { break }
        kept.append(scalar)
        byteCount += size
    }
    return String(kept)
}

func boundedFinalTranscript(_ text: String) -> String {
    String(String.UnicodeScalarView(text.unicodeScalars.prefix(2000)))
}

enum RecognitionRestartThrottle {
    static let minimumIntervalNanoseconds: UInt64 = 2_000_000_000
}

enum RecognitionSegmentLifecycle: Equatable {
    case accepting
    case ending
    case terminal
    case cancelling
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

struct RecognitionState<Session> {
    let source: AudioSource
    let session: Session
    let generation: Int
    var lifecycle: RecognitionSegmentLifecycle
    var sessionTerminalArrived: Bool
    var sessionCancellationRequested: Bool
    var closeReason: RecognitionSegmentCloseReason?
    var transcriptSequence: UInt64
}

struct RecognitionStateStore<Session> {
    typealias State = RecognitionState<Session>

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
        session: Session,
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
        let sessionTerminalArrived = terminalGenerations.remove(key) != nil
        states[source] = State(
            source: source,
            session: session,
            generation: generation,
            lifecycle: sessionTerminalArrived ? .terminal : .accepting,
            sessionTerminalArrived: sessionTerminalArrived,
            sessionCancellationRequested: false,
            closeReason: nil,
            transcriptSequence: 0
        )
        return true
    }

    mutating func nextTranscriptSequence(source: AudioSource, generation: Int) -> UInt64? {
        guard var state = states[source],
              isCurrentState(source, generation),
              state.lifecycle != .cancelling else {
            return nil
        }
        state.transcriptSequence += 1
        states[source] = state
        return state.transcriptSequence
    }

    mutating func markSessionTerminal(source: AudioSource, generation: Int) -> Bool {
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
              !state.sessionTerminalArrived else {
            return false
        }
        state.sessionTerminalArrived = true
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
              !state.sessionTerminalArrived else {
            return nil
        }
        state.lifecycle = .cancelling
        state.sessionCancellationRequested = true
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
              !state.sessionTerminalArrived else {
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

    func currentSession(for source: AudioSource) -> Session? {
        states[source]?.session
    }

    func currentGeneration(for source: AudioSource) -> Int? {
        states[source]?.generation
    }

    func sessionTerminalArrived(for source: AudioSource) -> Bool {
        states[source]?.sessionTerminalArrived == true
    }

    func sessionCancellationRequested(for source: AudioSource) -> Bool {
        states[source]?.sessionCancellationRequested == true
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
