enum VoiceActivityFinishReason: Equatable {
    case trailing
    case maximum
    case steadyNoise
}

enum VoiceActivityAction: Equatable {
    case wait
    case start
    case append
    case appendAndFinish(VoiceActivityFinishReason)
    case bufferForNextSegment
}

struct VoiceActivityConfiguration: Equatable {
    static let standard = VoiceActivityConfiguration(
        noiseFloorAdaptationRate: 0.05,
        startNoiseMultiplier: 6.0,
        sustainNoiseMultiplier: 2.5,
        minimumStartRmsThreshold: 0.002,
        minimumSustainRmsThreshold: 0.0008,
        movingRmsWindowNanoseconds: 150_000_000,
        maximumSegmentNanoseconds: 15_000_000_000,
        trailingNanoseconds: 1_200_000_000,
        startAttackNanoseconds: 150_000_000,
        rearmQuietNanoseconds: 400_000_000,
        preRollNanoseconds: 300_000_000,
        steadyNoiseWindowNanoseconds: 600_000_000,
        steadyNoiseDurationNanoseconds: 600_000_000,
        steadyNoiseMaximumCoefficientOfVariation: 0.08,
        steadyNoiseMaximumStartRatio: 2.5
    )

    let noiseFloorAdaptationRate: Double
    let startNoiseMultiplier: Double
    let sustainNoiseMultiplier: Double
    let minimumStartRmsThreshold: Double
    let minimumSustainRmsThreshold: Double
    let movingRmsWindowNanoseconds: UInt64
    let maximumSegmentNanoseconds: UInt64
    let trailingNanoseconds: UInt64
    let startAttackNanoseconds: UInt64
    let rearmQuietNanoseconds: UInt64
    let preRollNanoseconds: UInt64
    let steadyNoiseWindowNanoseconds: UInt64
    let steadyNoiseDurationNanoseconds: UInt64
    let steadyNoiseMaximumCoefficientOfVariation: Double
    let steadyNoiseMaximumStartRatio: Double
    let maximumStartRmsThreshold: Double
    let maximumSustainRmsThreshold: Double
    let maximumNoiseFloorRiseFractionPerSecond: Double

    init(
        noiseFloorAdaptationRate: Double,
        startNoiseMultiplier: Double,
        sustainNoiseMultiplier: Double,
        minimumStartRmsThreshold: Double,
        minimumSustainRmsThreshold: Double,
        movingRmsWindowNanoseconds: UInt64,
        maximumSegmentNanoseconds: UInt64,
        trailingNanoseconds: UInt64,
        startAttackNanoseconds: UInt64 = 150_000_000,
        rearmQuietNanoseconds: UInt64 = 400_000_000,
        preRollNanoseconds: UInt64 = 300_000_000,
        steadyNoiseWindowNanoseconds: UInt64 = 600_000_000,
        steadyNoiseDurationNanoseconds: UInt64 = 600_000_000,
        steadyNoiseMaximumCoefficientOfVariation: Double = 0.08,
        steadyNoiseMaximumStartRatio: Double = 1.8,
        maximumStartRmsThreshold: Double = 0.02,
        maximumSustainRmsThreshold: Double = 0.01,
        maximumNoiseFloorRiseFractionPerSecond: Double = 0.2
    ) {
        self.noiseFloorAdaptationRate = noiseFloorAdaptationRate
        self.startNoiseMultiplier = startNoiseMultiplier
        self.sustainNoiseMultiplier = sustainNoiseMultiplier
        self.minimumStartRmsThreshold = minimumStartRmsThreshold
        self.minimumSustainRmsThreshold = minimumSustainRmsThreshold
        self.movingRmsWindowNanoseconds = movingRmsWindowNanoseconds
        self.maximumSegmentNanoseconds = maximumSegmentNanoseconds
        self.trailingNanoseconds = trailingNanoseconds
        self.startAttackNanoseconds = startAttackNanoseconds
        self.rearmQuietNanoseconds = rearmQuietNanoseconds
        self.preRollNanoseconds = preRollNanoseconds
        self.steadyNoiseWindowNanoseconds = steadyNoiseWindowNanoseconds
        self.steadyNoiseDurationNanoseconds = steadyNoiseDurationNanoseconds
        self.steadyNoiseMaximumCoefficientOfVariation = steadyNoiseMaximumCoefficientOfVariation
        self.steadyNoiseMaximumStartRatio = steadyNoiseMaximumStartRatio
        self.maximumStartRmsThreshold = maximumStartRmsThreshold
        self.maximumSustainRmsThreshold = maximumSustainRmsThreshold
        self.maximumNoiseFloorRiseFractionPerSecond = maximumNoiseFloorRiseFractionPerSecond
    }
}

struct VoiceActivityLevels: Equatable {
    let noiseFloorRms: Double
    let startRmsThreshold: Double
    let sustainRmsThreshold: Double
}

struct MovingRmsWindow {
    private struct Sample {
        var rms: Double
        var durationNanoseconds: UInt64
    }

    private let capacityNanoseconds: UInt64
    private var samples: [Sample] = []
    private var totalDurationNanoseconds: UInt64 = 0
    private var totalSquareDuration: Double = 0
    private var totalRmsDuration: Double = 0

    init(capacityNanoseconds: UInt64) {
        precondition(capacityNanoseconds > 0)
        self.capacityNanoseconds = capacityNanoseconds
    }

    var rms: Double {
        guard totalDurationNanoseconds > 0 else { return 0 }
        return max(totalSquareDuration, 0).squareRoot()
            / Double(totalDurationNanoseconds).squareRoot()
    }

    var durationNanoseconds: UInt64 { totalDurationNanoseconds }

    var meanRms: Double {
        guard totalDurationNanoseconds > 0 else { return 0 }
        return totalRmsDuration / Double(totalDurationNanoseconds)
    }

    var coefficientOfVariation: Double {
        let mean = meanRms
        guard mean > 0, mean.isFinite else { return 0 }
        let variance = max(rms * rms - mean * mean, 0)
        return variance.squareRoot() / mean
    }

    mutating func append(rms: Double, durationNanoseconds: UInt64) {
        precondition(rms.isFinite && rms >= 0)
        precondition(durationNanoseconds > 0)
        let (newTotal, overflow) = totalDurationNanoseconds.addingReportingOverflow(
            durationNanoseconds
        )
        precondition(!overflow, "moving RMS duration overflowed")

        samples.append(Sample(rms: rms, durationNanoseconds: durationNanoseconds))
        totalDurationNanoseconds = newTotal
        totalSquareDuration += rms * rms * Double(durationNanoseconds)
        totalRmsDuration += rms * Double(durationNanoseconds)
        trimToCapacity()
    }

    mutating func reset() {
        samples.removeAll(keepingCapacity: true)
        totalDurationNanoseconds = 0
        totalSquareDuration = 0
        totalRmsDuration = 0
    }

    private mutating func trimToCapacity() {
        while totalDurationNanoseconds > capacityNanoseconds {
            let excess = totalDurationNanoseconds - capacityNanoseconds
            guard var first = samples.first else {
                preconditionFailure("moving RMS samples are missing")
            }
            if first.durationNanoseconds <= excess {
                samples.removeFirst()
                totalDurationNanoseconds -= first.durationNanoseconds
                totalSquareDuration -= first.rms * first.rms
                    * Double(first.durationNanoseconds)
                totalRmsDuration -= first.rms * Double(first.durationNanoseconds)
            } else {
                first.durationNanoseconds -= excess
                samples[0] = first
                totalDurationNanoseconds -= excess
                totalSquareDuration -= first.rms * first.rms * Double(excess)
                totalRmsDuration -= first.rms * Double(excess)
            }
        }
    }
}

enum VoiceActivityPhase: Equatable {
    case waiting
    case speaking(startedAt: UInt64, lastSpeechAt: UInt64)
    case finishing
    case pending
    case rearming(quietSince: UInt64?)
}

struct VoiceActivityDetector {
    private static let nonSpeechFloorFraction = 0.3
    private static let knownNoiseMaximumRatio = 1.5
    private static let rearmQuietStartRatio = 0.8
    private static let rearmNoiseToleranceRatio = 1.25

    private let configuration: VoiceActivityConfiguration
    private(set) var phase: VoiceActivityPhase = .waiting
    private(set) var noiseFloorRms: Double = 0
    private var movingRmsWindow: MovingRmsWindow
    private var steadyRmsWindow: MovingRmsWindow
    private var steadyNoiseDurationNanoseconds: UInt64 = 0
    private var startCandidateSince: UInt64?
    private var rearmNoiseReferenceRms: Double?
    private var noiseReferenceForRearmingRms: Double?

    init(configuration: VoiceActivityConfiguration) {
        self.configuration = configuration
        movingRmsWindow = MovingRmsWindow(
            capacityNanoseconds: configuration.movingRmsWindowNanoseconds
        )
        steadyRmsWindow = MovingRmsWindow(
            capacityNanoseconds: configuration.steadyNoiseWindowNanoseconds
        )
        startCandidateSince = nil
        rearmNoiseReferenceRms = nil
        noiseReferenceForRearmingRms = nil
    }

    var isSpeaking: Bool {
        if case .speaking = phase { return true }
        return false
    }

    var startRmsThreshold: Double {
        min(
            max(
                noiseFloorRms * configuration.startNoiseMultiplier,
                configuration.minimumStartRmsThreshold
            ),
            configuration.maximumStartRmsThreshold
        )
    }

    var sustainRmsThreshold: Double {
        min(
            max(
                noiseFloorRms * configuration.sustainNoiseMultiplier,
                configuration.minimumSustainRmsThreshold
            ),
            configuration.maximumSustainRmsThreshold
        )
    }

    var levels: VoiceActivityLevels {
        VoiceActivityLevels(
            noiseFloorRms: noiseFloorRms,
            startRmsThreshold: startRmsThreshold,
            sustainRmsThreshold: sustainRmsThreshold
        )
    }

    mutating func observe(
        rms: Double,
        durationNanoseconds: UInt64,
        at timestamp: UInt64
    ) -> VoiceActivityAction {
        precondition(rms.isFinite && rms >= 0)
        movingRmsWindow.append(rms: rms, durationNanoseconds: durationNanoseconds)

        switch phase {
        case .waiting:
            return observeWaiting(
                rms: rms,
                durationNanoseconds: durationNanoseconds,
                at: timestamp
            )
        case let .speaking(startedAt, lastSpeechAt):
            let segmentElapsed = timestamp >= startedAt ? timestamp - startedAt : 0
            if segmentElapsed >= configuration.maximumSegmentNanoseconds {
                let noiseReference = nonSpeechRmsReference
                updateNoiseFloorAfterNonSpeech(
                    noiseReference,
                    durationNanoseconds: max(segmentElapsed, durationNanoseconds)
                )
                noiseReferenceForRearmingRms = movingRmsWindow.coefficientOfVariation
                    <= configuration.steadyNoiseMaximumCoefficientOfVariation
                    ? noiseReference
                    : nil
                phase = .finishing
                return .appendAndFinish(.maximum)
            }
            updateSteadyNoiseState(
                rms: rms,
                durationNanoseconds: durationNanoseconds
            )
            if steadyNoiseDurationNanoseconds >= configuration.steadyNoiseDurationNanoseconds {
                let noiseReference = nonSpeechRmsReference
                updateNoiseFloorAfterNonSpeech(
                    noiseReference,
                    durationNanoseconds: max(
                        steadyRmsWindow.durationNanoseconds,
                        durationNanoseconds
                    )
                )
                noiseReferenceForRearmingRms = noiseReference
                phase = .finishing
                steadyNoiseDurationNanoseconds = 0
                return .appendAndFinish(.steadyNoise)
            }
            if movingRmsWindow.rms >= sustainRmsThreshold {
                phase = .speaking(startedAt: startedAt, lastSpeechAt: timestamp)
                return .append
            }
            let elapsed = timestamp >= lastSpeechAt ? timestamp - lastSpeechAt : 0
            guard elapsed >= configuration.trailingNanoseconds else { return .append }
            noiseReferenceForRearmingRms = nil
            phase = .finishing
            return .appendAndFinish(.trailing)
        case .finishing:
            guard rms >= startRmsThreshold else { return .wait }
            phase = .pending
            return .bufferForNextSegment
        case .pending:
            return .bufferForNextSegment
        case let .rearming(quietSince):
            return observeRearming(
                rms: rms,
                durationNanoseconds: durationNanoseconds,
                at: timestamp,
                quietSince: quietSince
            )
        }
    }

    mutating func finishSegment(at timestamp: UInt64, rearmImmediately: Bool = false) {
        let noiseReference = noiseReferenceForRearmingRms
        noiseReferenceForRearmingRms = nil
        rearmNoiseReferenceRms = rearmImmediately ? nil : noiseReference
        phase = rearmImmediately ? .waiting : .rearming(quietSince: timestamp)
        movingRmsWindow.reset()
        steadyRmsWindow.reset()
        steadyNoiseDurationNanoseconds = 0
        startCandidateSince = nil
    }

    mutating func resetToWaiting() {
        phase = .waiting
        movingRmsWindow.reset()
        steadyRmsWindow.reset()
        steadyNoiseDurationNanoseconds = 0
        startCandidateSince = nil
        rearmNoiseReferenceRms = nil
        noiseReferenceForRearmingRms = nil
    }

    mutating func forceFinish() -> Bool {
        guard case .speaking = phase else { return false }
        phase = .finishing
        steadyRmsWindow.reset()
        steadyNoiseDurationNanoseconds = 0
        noiseReferenceForRearmingRms = nil
        return true
    }

    mutating func deferCurrentSegment() {
        guard case .speaking = phase else { return }
        phase = .pending
        steadyRmsWindow.reset()
        steadyNoiseDurationNanoseconds = 0
        startCandidateSince = nil
    }

    private mutating func observeRearming(
        rms: Double,
        durationNanoseconds: UInt64,
        at timestamp: UInt64,
        quietSince: UInt64?
    ) -> VoiceActivityAction {
        let quietThreshold = rearmQuietRmsThreshold
        let movingRms = movingRmsWindow.rms
        if rms <= quietThreshold, movingRms <= quietThreshold {
            startCandidateSince = nil
            let quietStart = quietSince ?? timestamp
            let quietDuration = timestamp >= quietStart
                ? timestamp - quietStart + durationNanoseconds
                : durationNanoseconds
            guard quietDuration >= configuration.rearmQuietNanoseconds else {
                phase = .rearming(quietSince: quietStart)
                return .wait
            }
            phase = .waiting
            return .wait
        }

        let startThreshold = startRmsThreshold
        guard rms >= startThreshold, movingRms >= startThreshold else {
            startCandidateSince = nil
            phase = .rearming(quietSince: nil)
            return .wait
        }
        let candidateSince = startCandidateSince ?? timestamp
        startCandidateSince = candidateSince
        let candidateDuration = timestamp >= candidateSince
            ? timestamp - candidateSince + durationNanoseconds
            : durationNanoseconds
        guard candidateDuration >= configuration.startAttackNanoseconds else {
            phase = .rearming(quietSince: nil)
            return .wait
        }
        phase = .speaking(startedAt: candidateSince, lastSpeechAt: timestamp)
        startCandidateSince = nil
        rearmNoiseReferenceRms = nil
        return .start
    }

    private var rearmQuietRmsThreshold: Double {
        let baseline = max(
            sustainRmsThreshold,
            startRmsThreshold * Self.rearmQuietStartRatio
        )
        guard let rearmNoiseReferenceRms else { return baseline }
        return max(
            baseline,
            rearmNoiseReferenceRms * Self.rearmNoiseToleranceRatio
        )
    }

    private var nonSpeechRmsReference: Double {
        max(steadyRmsWindow.meanRms, movingRmsWindow.meanRms)
    }

    private mutating func updateNoiseFloorAfterNonSpeech(
        _ rms: Double,
        durationNanoseconds: UInt64
    ) {
        guard rms.isFinite, rms > 0 else { return }
        updateNoiseFloor(
            with: rms * Self.nonSpeechFloorFraction,
            durationNanoseconds: durationNanoseconds
        )
    }

    private mutating func updateSteadyNoiseState(
        rms: Double,
        durationNanoseconds: UInt64
    ) {
        let startThreshold = startRmsThreshold
        let looksSteady = rms <= startThreshold * configuration.steadyNoiseMaximumStartRatio
        if looksSteady {
            steadyRmsWindow.append(
                rms: rms,
                durationNanoseconds: durationNanoseconds
            )
        } else {
            steadyRmsWindow.reset()
            steadyNoiseDurationNanoseconds = 0
            return
        }

        let isSteady = steadyRmsWindow.durationNanoseconds
                >= configuration.steadyNoiseWindowNanoseconds
            && steadyRmsWindow.coefficientOfVariation
                <= configuration.steadyNoiseMaximumCoefficientOfVariation
        if isSteady {
            let (duration, overflow) = steadyNoiseDurationNanoseconds
                .addingReportingOverflow(durationNanoseconds)
            steadyNoiseDurationNanoseconds = overflow ? UInt64.max : duration
        } else {
            steadyNoiseDurationNanoseconds = 0
        }
    }

    private mutating func observeWaiting(
        rms: Double,
        durationNanoseconds: UInt64,
        at timestamp: UInt64
    ) -> VoiceActivityAction {
        if let rearmNoiseReferenceRms,
           rms <= rearmNoiseReferenceRms * Self.knownNoiseMaximumRatio,
           movingRmsWindow.rms <= rearmNoiseReferenceRms * Self.knownNoiseMaximumRatio {
            startCandidateSince = nil
            updateNoiseFloorAfterNonSpeech(
                rearmNoiseReferenceRms,
                durationNanoseconds: durationNanoseconds
            )
            return .wait
        }

        rearmNoiseReferenceRms = nil
        let startThreshold = startRmsThreshold
        guard rms >= startThreshold, movingRmsWindow.rms >= startThreshold else {
            startCandidateSince = nil
            if rms < sustainRmsThreshold,
               movingRmsWindow.rms < sustainRmsThreshold,
               movingRmsWindow.durationNanoseconds
                   >= configuration.movingRmsWindowNanoseconds {
                updateNoiseFloor(
                    with: rms,
                    durationNanoseconds: durationNanoseconds
                )
            }
            return .wait
        }

        let candidateSince = startCandidateSince ?? timestamp
        startCandidateSince = candidateSince
        let candidateDuration = timestamp >= candidateSince
            ? timestamp - candidateSince + durationNanoseconds
            : durationNanoseconds
        guard candidateDuration >= configuration.startAttackNanoseconds else {
            return .wait
        }
        phase = .speaking(startedAt: candidateSince, lastSpeechAt: timestamp)
        startCandidateSince = nil
        return .start
    }

    private mutating func updateNoiseFloor(
        with rms: Double,
        durationNanoseconds: UInt64
    ) {
        let adaptationRate = configuration.noiseFloorAdaptationRate
        let target = noiseFloorRms + adaptationRate * (rms - noiseFloorRms)
        guard target > noiseFloorRms else {
            noiseFloorRms = target
            return
        }

        let durationSeconds = Double(durationNanoseconds) / 1_000_000_000
        let referenceFloor = max(
            noiseFloorRms,
            configuration.minimumSustainRmsThreshold
        )
        let maximumIncrease = referenceFloor
            * configuration.maximumNoiseFloorRiseFractionPerSecond
            * durationSeconds
        noiseFloorRms += min(target - noiseFloorRms, maximumIncrease)
    }
}
