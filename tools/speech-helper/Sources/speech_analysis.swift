import AVFoundation
import CoreMedia

struct SpeechAnalysisFailure: Error {
    let kind: String
    let message: String
}

struct SpeechWordTiming: Equatable {
    let text: String
    let start: CMTime
    let duration: CMTime

    var end: CMTime { CMTimeAdd(start, duration) }
}

struct SpeechTranscription {
    let text: String
    let audioRange: CMTimeRange
    let resultsFinalizationTime: CMTime
    let words: [SpeechWordTiming]

    init(
        text: String,
        audioRange: CMTimeRange,
        resultsFinalizationTime: CMTime,
        words: [SpeechWordTiming] = []
    ) {
        self.text = text
        self.audioRange = audioRange
        self.resultsFinalizationTime = resultsFinalizationTime
        self.words = words
    }

    var isFinal: Bool { CMTimeCompare(resultsFinalizationTime, audioRange.end) >= 0 }
}

enum SpeechAnalysisEvent {
    case ready
    case result(SpeechTranscription)
    case finalizedThrough(CMTime)
    case completed
    case failed(SpeechAnalysisFailure)
    case cancelled
}

protocol SpeechAnalysis: AnyObject {
    func start(receive: @escaping (SpeechAnalysisEvent) -> Void)
    func append(_ buffer: AVAudioPCMBuffer) throws
    func finish()
    func cancel()
}
