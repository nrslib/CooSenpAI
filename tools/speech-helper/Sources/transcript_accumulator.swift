import CoreMedia
import Foundation

struct SpeechTranscription {
    let text: String
    let audioRange: CMTimeRange
    let resultsFinalizationTime: CMTime

    var isFinal: Bool { CMTimeCompare(resultsFinalizationTime, audioRange.end) >= 0 }
}

enum SpeechTranscriptError: Error {
    case invalidAudioRange
    case invalidFinalizationTime
    case overlapsFinalizedAudio
}

struct SpeechTranscript {
    private var committed = ""
    private var committedEnd = CMTime.zero
    private var pending: [SpeechTranscription] = []
    private let separator: String

    init(locale: Locale) {
        switch locale.language.languageCode?.identifier {
        case "ja", "zh", "th", "lo", "km", "my": separator = ""
        default: separator = " "
        }
    }

    var text: String { pending.reduce(committed) { join($0, $1.text) } }
    var finalizedText: String { committed }
    var hasUnfinalizedText: Bool {
        pending.contains { !$0.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    mutating func record(_ transcription: SpeechTranscription) throws {
        let range = transcription.audioRange
        guard range.isValid, range.start.isNumeric, range.duration.isNumeric, range.end.isNumeric,
              CMTimeCompare(range.start, .zero) >= 0,
              CMTimeCompare(range.duration, .zero) > 0 else {
            throw SpeechTranscriptError.invalidAudioRange
        }
        try validateFinalizationTime(transcription.resultsFinalizationTime)
        if CMTimeCompare(range.end, committedEnd) > 0 {
            guard CMTimeCompare(range.start, committedEnd) >= 0 else {
                throw SpeechTranscriptError.overlapsFinalizedAudio
            }
            pending.removeAll {
                CMTimeCompare($0.audioRange.start, range.end) < 0 &&
                CMTimeCompare(range.start, $0.audioRange.end) < 0
            }
            pending.append(transcription)
            pending.sort { CMTimeCompare($0.audioRange.start, $1.audioRange.start) < 0 }
        }
        try finalize(through: transcription.resultsFinalizationTime)
    }

    mutating func finalize(through time: CMTime) throws {
        try validateFinalizationTime(time)
        let count = pending.prefix { CMTimeCompare($0.audioRange.end, time) <= 0 }.count
        for result in pending.prefix(count) {
            committed = join(committed, result.text)
            committedEnd = result.audioRange.end
        }
        pending.removeFirst(count)
    }

    private func validateFinalizationTime(_ time: CMTime) throws {
        guard time.isNumeric, CMTimeCompare(time, .zero) >= 0 else {
            throw SpeechTranscriptError.invalidFinalizationTime
        }
    }

    private func join(_ prefix: String, _ suffix: String) -> String {
        guard let last = prefix.last, let first = suffix.first else { return prefix + suffix }
        let boundary = last.isWhitespace || first.isWhitespace ? "" : separator
        return prefix + boundary + suffix
    }
}
