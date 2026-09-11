import AVFoundation
import Foundation
import Speech

// SFSpeechRecognizer の結果は区間差分ではなく、その時点の認識全文として扱う。
final class OnDeviceSpeechRecognizer: SpeechAnalysis, @unchecked Sendable {
    private let locale: Locale
    private let diagnostic: (String) -> Void
    private var receive: ((SpeechAnalysisEvent) -> Void)?
    private var recognizer: SFSpeechRecognizer?
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?
    private var latestText = ""
    private var finishRequested = false
    private var inputEnded = false
    private var terminal = false

    init(locale: Locale, diagnostic: @escaping (String) -> Void) {
        self.locale = locale
        self.diagnostic = diagnostic
    }

    func start(receive: @escaping (SpeechAnalysisEvent) -> Void) {
        self.receive = receive
        let authorization = SFSpeechRecognizer.authorizationStatus()
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
            receive?(.failed(SpeechAnalysisFailure(kind: "permission-speech", message: "音声認識の使用が許可されていません")))
            return
        }
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            receive?(.failed(SpeechAnalysisFailure(kind: "locale-unavailable", message: "指定したロケールの音声認識は利用できません: \(locale.identifier)")))
            return
        }
        guard recognizer.supportsOnDeviceRecognition else {
            receive?(.failed(SpeechAnalysisFailure(kind: "on-device-unsupported", message: "指定したロケールはオンデバイス音声認識に対応していません: \(locale.identifier)")))
            return
        }
        self.recognizer = recognizer
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = true
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
            // 旧経路と同じく、終了通知だけの空結果で最後の認識全文を消さない。
            if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                latestText = text
            }
            if result.isFinal {
                complete(latestText)
            } else {
                receive?(.partialTranscript(text))
            }
        } else if let error {
            let details = error as NSError
            if inputEnded, latestText.isEmpty,
               details.domain == "kAFAssistantErrorDomain", details.code == 1110 {
                diagnostic("event=analysis-no-speech engine=SFSpeechRecognizer")
                complete("")
                return
            }
            diagnostic("event=analysis-error engine=SFSpeechRecognizer domain=\(details.domain) code=\(details.code)")
            receive?(.failed(SpeechAnalysisFailure(kind: "recognition", message: "音声認識が正常に完了しませんでした")))
        }
    }

    private func complete(_ text: String) {
        terminal = true
        request = nil
        task = nil
        recognizer = nil
        receive?(.completedTranscript(text))
    }

    func cancel() {
        guard !terminal else {
            receive?(.cancelled)
            return
        }
        terminal = true
        request?.endAudio()
        task?.cancel()
        request = nil
        task = nil
        recognizer = nil
        receive?(.cancelled)
    }
}
