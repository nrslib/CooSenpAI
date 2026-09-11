import AVFoundation
import CoreMedia
import Foundation
import SoundAnalysis

let speakerMusicGateAnalysisWindowNanoseconds: UInt64 = 1_000_000_000
let speakerMusicGateMusicConfidenceThreshold = 0.70
let speakerMusicGateSpeechConfidenceThreshold = 0.40

struct SpeakerMusicClassification: Equatable {
    let musicConfidence: Double
    let speechConfidence: Double
}

enum SpeakerMusicGateDecision: String, Equatable {
    case pending
    case pass
    case suppress
}

enum SpeakerMusicGatePolicy {
    static func decision(
        for classification: SpeakerMusicClassification
    ) -> SpeakerMusicGateDecision {
        if classification.speechConfidence >= speakerMusicGateSpeechConfidenceThreshold {
            return .pass
        }
        if classification.musicConfidence >= speakerMusicGateMusicConfidenceThreshold {
            return .suppress
        }
        return .pass
    }
}

enum SoundAnalysisMusicClassifierError: LocalizedError {
    case requiredClassificationUnavailable
    case audioDurationUnavailable
    case framePositionOverflow

    var errorDescription: String? {
        switch self {
        case .requiredClassificationUnavailable:
            return "SoundAnalysis のビルトイン分類器に music または speech がありません"
        case .audioDurationUnavailable:
            return "SoundAnalysis 用の音声バッファの時間を計算できません"
        case .framePositionOverflow:
            return "SoundAnalysis の音声フレーム位置がオーバーフローしました"
        }
    }
}

final class SoundAnalysisMusicClassifier: NSObject, SNResultsObserving {
    private static let musicLabel = "music"
    private static let speechLabel = "speech"

    private let analyzer: SNAudioStreamAnalyzer
    private let request: SNClassifySoundRequest
    private let onClassification: (SpeakerMusicClassification) -> Void
    private let onFailure: (Error) -> Void
    private let onComplete: () -> Void
    private var nextFramePosition: AVAudioFramePosition = 0
    private var analysisCompleted = false

    init(
        format: AVAudioFormat,
        onClassification: @escaping (SpeakerMusicClassification) -> Void,
        onFailure: @escaping (Error) -> Void,
        onComplete: @escaping () -> Void
    ) throws {
        let request = try SNClassifySoundRequest(classifierIdentifier: .version1)
        guard Set(request.knownClassifications).isSuperset(
            of: [Self.musicLabel, Self.speechLabel]
        ) else {
            throw SoundAnalysisMusicClassifierError.requiredClassificationUnavailable
        }
        request.windowDuration = CMTime(
            seconds: 1.0,
            preferredTimescale: 1_000
        )
        request.overlapFactor = 0.0
        let analyzer = SNAudioStreamAnalyzer(format: format)
        self.analyzer = analyzer
        self.request = request
        self.onClassification = onClassification
        self.onFailure = onFailure
        self.onComplete = onComplete
        super.init()
        try analyzer.add(request, withObserver: self)
    }

    func analyze(_ buffer: AVAudioPCMBuffer) {
        guard !analysisCompleted else { return }
        let frameCount = AVAudioFramePosition(buffer.frameLength)
        let (nextPosition, overflow) = nextFramePosition.addingReportingOverflow(frameCount)
        guard !overflow else {
            analysisCompleted = true
            onFailure(SoundAnalysisMusicClassifierError.framePositionOverflow)
            return
        }
        let framePosition = nextFramePosition
        nextFramePosition = nextPosition
        analyzer.analyze(buffer, atAudioFramePosition: framePosition)
    }

    func complete() {
        guard !analysisCompleted else { return }
        analysisCompleted = true
        analyzer.completeAnalysis()
    }

    func request(_ request: SNRequest, didProduce result: SNResult) {
        guard let result = result as? SNClassificationResult else { return }
        onClassification(
            SpeakerMusicClassification(
                musicConfidence: result.classification(forIdentifier: Self.musicLabel)?.confidence
                    ?? 0,
                speechConfidence: result.classification(forIdentifier: Self.speechLabel)?.confidence
                    ?? 0
            )
        )
    }

    func request(_ request: SNRequest, didFailWithError error: Error) {
        onFailure(error)
    }

    func requestDidComplete(_ request: SNRequest) {
        onComplete()
    }
}

final class SpeakerMusicGateSegment {
    private let classifier: SoundAnalysisMusicClassifier
    private(set) var decision: SpeakerMusicGateDecision = .pending
    private(set) var closeReason: RecognitionSegmentCloseReason?
    private var bufferedAudio: [PendingAudioBuffer] = []
    private var bufferedDurationNanoseconds: UInt64 = 0
    private var analysisCompleted = false

    init(
        format: AVAudioFormat,
        onClassification: @escaping (SpeakerMusicClassification) -> Void,
        onFailure: @escaping (Error) -> Void,
        onComplete: @escaping () -> Void
    ) throws {
        classifier = try SoundAnalysisMusicClassifier(
            format: format,
            onClassification: onClassification,
            onFailure: onFailure,
            onComplete: onComplete
        )
    }

    func append(_ audio: PendingAudioBuffer, durationNanoseconds: UInt64) {
        guard decision == .pending, closeReason == nil else {
            preconditionFailure("music gate segment is no longer accepting audio")
        }
        precondition(durationNanoseconds > 0)
        bufferedAudio.append(audio)
        let (newDuration, overflow) = bufferedDurationNanoseconds.addingReportingOverflow(
            durationNanoseconds
        )
        precondition(!overflow, "music gate audio duration overflowed")
        bufferedDurationNanoseconds = newDuration
        classifier.analyze(audio.buffer)
        if bufferedDurationNanoseconds >= speakerMusicGateAnalysisWindowNanoseconds {
            completeAnalysisIfNeeded()
        }
    }

    func requestClose(reason: RecognitionSegmentCloseReason) {
        guard closeReason == nil else { return }
        closeReason = reason
        completeAnalysisIfNeeded()
    }

    @discardableResult
    func resolve(_ classification: SpeakerMusicClassification?) -> SpeakerMusicGateDecision {
        guard decision == .pending else { return decision }
        decision = classification.map(SpeakerMusicGatePolicy.decision) ?? .pass
        return decision
    }

    func takeBufferedAudio() -> [PendingAudioBuffer] {
        let audio = bufferedAudio
        bufferedAudio.removeAll(keepingCapacity: false)
        bufferedDurationNanoseconds = 0
        return audio
    }

    private func completeAnalysisIfNeeded() {
        guard !analysisCompleted else { return }
        analysisCompleted = true
        classifier.complete()
    }
}
