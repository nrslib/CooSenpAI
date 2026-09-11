import AVFoundation
import Foundation

enum SpeechEngine: String {
    case analyzer
    case sf

    var name: String {
        switch self {
        case .analyzer: return "SpeechAnalyzer"
        case .sf: return "SFSpeechRecognizer"
        }
    }

    static func resolve(_ value: String) throws -> SpeechEngine {
        switch value {
        case "auto":
            if #available(macOS 26.0, *) { return .analyzer }
            return .sf
        case "sf": return .sf
        case "analyzer":
            if #available(macOS 26.0, *) { return .analyzer }
            throw SpeechAnalysisFailure(kind: "arguments", message: "--engine analyzer には macOS 26.0 以降が必要です")
        default:
            throw SpeechAnalysisFailure(kind: "arguments", message: "--engine は auto / analyzer / sf を指定してください")
        }
    }

    func makeAnalysis(
        locale: Locale, inputFormat: AVAudioFormat, traceResults: Bool,
        diagnostic: @escaping (String) -> Void
    ) -> SpeechAnalysis {
        switch self {
        case .analyzer:
            if #available(macOS 26.0, *) {
                return OnDeviceSpeechAnalyzer(
                    locale: locale, inputFormat: inputFormat,
                    traceResults: traceResults, diagnostic: diagnostic
                )
            }
            preconditionFailure("SpeechEngine.resolve must reject unavailable engines")
        case .sf:
            return OnDeviceSpeechRecognizer(locale: locale, diagnostic: diagnostic)
        }
    }
}
