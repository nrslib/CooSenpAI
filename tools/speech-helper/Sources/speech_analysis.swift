import AVFoundation
import CoreMedia

struct SpeechAnalysisFailure: Error {
    let kind: String
    let message: String
}

struct SpeechTranscription {
    let text: String
    let audioRange: CMTimeRange
    let resultsFinalizationTime: CMTime

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
