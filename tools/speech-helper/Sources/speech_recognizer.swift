import AVFoundation
import CoreMedia
import Foundation
import Speech

struct SpeechRecognitionSegment {
    let text: String
    let timestamp: TimeInterval
    let duration: TimeInterval
}

// SFSpeechRecognizer の結果も音声区間へ変換し、SpeechAnalyzer と同じ蓄積器へ渡す。
final class OnDeviceSpeechRecognizer: SpeechAnalysis, @unchecked Sendable {
    private static let timeScale: CMTimeScale = 1_000_000

    private let locale: Locale
    private let diagnostic: (String) -> Void
    private var receive: ((SpeechAnalysisEvent) -> Void)?
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var latestRangeEnd: CMTime?
    private var finishRequested = false
    private var inputEnded = false
    private var terminal = false
    private var reportedInputGain = false
    private var cancellationReported = false

    init(locale: Locale, diagnostic: @escaping (String) -> Void) {
        self.locale = locale
        self.diagnostic = diagnostic
    }

    func start(receive: @escaping (SpeechAnalysisEvent) -> Void) {
        self.receive = receive
        let authorization = SFSpeechRecognizer.authorizationStatus()
        diagnostic("event=analysis-authorization engine=SFSpeechRecognizer status=\(String(describing: authorization))")
        if authorization == .notDetermined {
            SFSpeechRecognizer.requestAuthorization { [weak self] status in
                DispatchQueue.main.async { self?.prepare(status) }
            }
        } else {
            prepare(authorization)
        }
    }

    private func prepare(_ authorization: SFSpeechRecognizerAuthorizationStatus) {
        guard !terminal else { return }
        guard authorization == .authorized else {
            reportFailure(SpeechAnalysisFailure(kind: "permission-speech", message: "音声認識の使用が許可されていません"))
            return
        }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            reportFailure(SpeechAnalysisFailure(kind: "locale-unavailable", message: "指定したロケールの音声認識は利用できません: \(locale.identifier)"))
            return
        }
        guard recognizer.supportsOnDeviceRecognition else {
            reportFailure(SpeechAnalysisFailure(kind: "on-device-unsupported", message: "指定したロケールはオンデバイス音声認識に対応していません: \(locale.identifier)"))
            return
        }
        self.recognizer = recognizer
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = true
        diagnostic("event=analysis-config engine=SFSpeechRecognizer shouldReportPartialResults=true requiresOnDeviceRecognition=true")
        self.request = request
        task = recognizer.recognitionTask(with: request) { [weak self] result, error in
            DispatchQueue.main.async { self?.handle(result, error: error) }
        }
        diagnostic("event=analysis-start engine=SFSpeechRecognizer")
        if finishRequested { finish() }
        receive?(.ready)
    }

    func append(_ buffer: AVAudioPCMBuffer) throws {
        guard !terminal, !finishRequested, let request else {
            throw SpeechAudioConversionError.inputClosed
        }
        let gainResult = try SpeechAudioGain.apply(to: buffer)
        if gainResult.inputPeak > 0, !reportedInputGain {
            reportedInputGain = true
            diagnostic(
                "event=analysis-input-gain engine=SFSpeechRecognizer "
                    + "inputPeak=\(gainResult.inputPeak) gain=\(gainResult.gain) "
                    + "targetPeak=\(SpeechAudioGain.targetPeak) maxGain=\(SpeechAudioGain.maximumGain)"
            )
        }
        request.append(buffer)
    }

    func finish() {
        guard !terminal else { return }
        finishRequested = true
        guard !inputEnded, let request else { return }
        inputEnded = true
        request.endAudio()
        diagnostic("event=recognizer-input-ended engine=SFSpeechRecognizer")
    }

    private func handle(_ result: SFSpeechRecognitionResult?, error: Error?) {
        guard !terminal else { return }
        if let result {
            let text = result.bestTranscription.formattedString
            diagnostic("event=analysis-result engine=SFSpeechRecognizer isFinal=\(result.isFinal) finishRequested=\(inputEnded) chars=\(text.count)")
            do {
                if let transcription = try makeTranscription(from: result) {
                    rememberRangeEnd(transcription.audioRange.end)
                    receive?(.result(transcription))
                } else if result.isFinal, let latestRangeEnd {
                    receive?(.finalizedThrough(latestRangeEnd))
                }
            } catch let failure as SpeechAnalysisFailure {
                reportFailure(failure)
                return
            } catch {
                reportFailure(SpeechAnalysisFailure(kind: "recognition", message: "音声認識が正常に完了しませんでした"))
                return
            }
            if result.isFinal {
                finishRecognition()
            }
        } else if let error {
            let details = error as NSError
            if inputEnded, details.domain == "kAFAssistantErrorDomain", details.code == 1110 {
                diagnostic("event=analysis-no-speech engine=SFSpeechRecognizer")
                if let latestRangeEnd { receive?(.finalizedThrough(latestRangeEnd)) }
                finishRecognition()
                return
            }
            diagnostic("event=analysis-error engine=SFSpeechRecognizer domain=\(details.domain) code=\(details.code)")
            reportFailure(SpeechAnalysisFailure(kind: "recognition", message: "音声認識が正常に完了しませんでした"))
        }
    }

    private func rememberRangeEnd(_ end: CMTime) {
        if let latestRangeEnd, CMTimeCompare(latestRangeEnd, end) >= 0 { return }
        latestRangeEnd = end
    }

    private func makeTranscription(from result: SFSpeechRecognitionResult) throws -> SpeechTranscription? {
        let transcription = result.bestTranscription
        let segments = transcription.segments.map {
            SpeechRecognitionSegment(
                text: $0.substring,
                timestamp: $0.timestamp,
                duration: $0.duration
            )
        }
        return try Self.transcription(text: transcription.formattedString, segments: segments, isFinal: result.isFinal)
    }

    static func transcription(
        text: String,
        segments: [SpeechRecognitionSegment],
        isFinal: Bool
    ) throws -> SpeechTranscription? {
        var words: [SpeechWordTiming] = []
        let ranges = try segments.compactMap { segment -> CMTimeRange? in
            guard segment.timestamp.isFinite, segment.duration.isFinite,
                  segment.timestamp >= 0, segment.duration >= 0 else {
                throw SpeechAnalysisFailure(kind: "recognition", message: "音声認識が不正な区間の結果を返しました")
            }
            let start = CMTime(seconds: segment.timestamp, preferredTimescale: Self.timeScale)
            let duration = CMTime(seconds: segment.duration, preferredTimescale: Self.timeScale)
            let end = CMTimeAdd(start, duration)
            guard start.isNumeric, duration.isNumeric, end.isNumeric,
                  CMTimeCompare(start, .zero) >= 0, CMTimeCompare(duration, .zero) >= 0,
                  CMTimeCompare(end, start) >= 0 else {
                throw SpeechAnalysisFailure(kind: "recognition", message: "音声認識が不正な区間の結果を返しました")
            }
            guard CMTimeCompare(end, start) > 0 else { return nil }
            words.append(SpeechWordTiming(text: segment.text, start: start, duration: duration))
            return CMTimeRange(start: start, end: end)
        }
        guard let firstRange = ranges.first else { return nil }
        let start = ranges.dropFirst().reduce(firstRange.start) {
            CMTimeCompare($1.start, $0) < 0 ? $1.start : $0
        }
        let end = ranges.dropFirst().reduce(firstRange.end) {
            CMTimeCompare($1.end, $0) > 0 ? $1.end : $0
        }
        guard CMTimeCompare(end, start) > 0 else {
            return nil
        }
        guard start.isNumeric, end.isNumeric, CMTimeCompare(start, .zero) >= 0, CMTimeCompare(end, start) > 0 else {
            throw SpeechAnalysisFailure(kind: "recognition", message: "音声認識が不正な区間の結果を返しました")
        }
        return SpeechTranscription(
            text: text,
            audioRange: CMTimeRange(start: start, end: end),
            resultsFinalizationTime: isFinal ? end : .zero,
            words: words
        )
    }

    private func finishRecognition() {
        guard !terminal else { return }
        terminal = true
        cleanupResources()
        receive?(.completed)
    }

    private func cleanupResources() {
        if !inputEnded {
            inputEnded = true
            request?.endAudio()
        }
        task?.cancel()
        request = nil
        task = nil
        recognizer = nil
    }

    private func reportFailure(_ failure: SpeechAnalysisFailure) {
        guard !terminal else { return }
        terminal = true
        cleanupResources()
        receive?(.failed(failure))
    }

    func cancel() {
        terminal = true
        cleanupResources()
        guard !cancellationReported else { return }
        cancellationReported = true
        receive?(.cancelled)
    }
}
