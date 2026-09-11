import Foundation

enum SpeakerBackend: String {
    case processTap = "process-tap"
    case screenCaptureKit = "screen-capture-kit"

    static func select(_ value: String = "auto") throws -> SpeakerBackend {
        switch value {
        case "auto":
            if #available(macOS 14.2, *) { return .processTap }
            return .screenCaptureKit
        case "screen-capture-kit": return .screenCaptureKit
        case "process-tap":
            if #available(macOS 14.2, *) { return .processTap }
            throw SelectionError.unavailable
        default: throw SelectionError.invalid
        }
    }

    enum SelectionError: LocalizedError {
        case unavailable
        case invalid

        var errorDescription: String? {
            switch self {
            case .unavailable: return "process-tap には macOS 14.2 以降が必要です"
            case .invalid: return "--speaker-backend は auto / process-tap / screen-capture-kit を指定してください"
            }
        }
    }
}
