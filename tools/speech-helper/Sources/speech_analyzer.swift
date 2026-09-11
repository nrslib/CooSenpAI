import AVFoundation
import CoreMedia
import Foundation
import Speech

// 公開操作と結果処理を main queue で直列化し、AVAudioConverter を並行使用しない。
@available(macOS 26.0, *)
final class OnDeviceSpeechAnalyzer: SpeechAnalysis, @unchecked Sendable {
    private let locale: Locale
    private let inputFormat: AVAudioFormat
    private let traceResults: Bool
    private let diagnostic: (String) -> Void
    private var receive: ((SpeechAnalysisEvent) -> Void)?
    private var analyzer: SpeechAnalyzer?
    private var converter: SpeechAudioConverter?
    private var continuation: AsyncStream<AnalyzerInput>.Continuation?
    private var preparationTask: Task<Void, Never>?
    private var resultTask: Task<Void, Error>?
    private var finishTask: Task<Void, Never>?
    private var finishRequested = false
    private var prepared = false
    private var inputEnded = false
    private var cancelled = false
    private var failed = false
    private var outputFrames: AVAudioFramePosition = 0

    init(locale: Locale, inputFormat: AVAudioFormat, traceResults: Bool, diagnostic: @escaping (String) -> Void) {
        self.locale = locale
        self.inputFormat = inputFormat
        self.traceResults = traceResults
        self.diagnostic = diagnostic
    }

    func start(receive: @escaping (SpeechAnalysisEvent) -> Void) {
        self.receive = receive
        preparationTask = Task { @MainActor in
            do {
                try await prepare()
                try Task.checkCancellation()
                prepared = true
                if finishRequested { finishInput() }
                receive(.ready)
            } catch {
                reportFailure(error)
            }
        }
    }

    @MainActor
    private func prepare() async throws {
        guard SpeechTranscriber.isAvailable else {
            throw SpeechAnalysisFailure(kind: "on-device-unsupported", message: "この Mac では SpeechTranscriber を利用できません")
        }
        guard let supportedLocale = await SpeechTranscriber.supportedLocale(equivalentTo: locale) else {
            throw SpeechAnalysisFailure(kind: "locale-unavailable", message: "指定したロケールの音声認識は利用できません: \(locale.identifier)")
        }
        try Task.checkCancellation()
        let transcriber = SpeechTranscriber(
            locale: supportedLocale,
            transcriptionOptions: [],
            reportingOptions: [.volatileResults, .fastResults],
            attributeOptions: []
        )
        if await AssetInventory.status(forModules: [transcriber]) != .installed {
            diagnostic("event=analysis-model-preparing locale=\(supportedLocale.identifier)")
            if let request = try await AssetInventory.assetInstallationRequest(supporting: [transcriber]) {
                try await request.downloadAndInstall()
            }
        }
        try Task.checkCancellation()
        guard await AssetInventory.status(forModules: [transcriber]) == .installed else {
            throw SpeechAnalysisFailure(kind: "recognition", message: "音声認識モデルを準備できませんでした")
        }
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber], considering: inputFormat) else {
            throw SpeechAnalysisFailure(kind: "audio", message: "音声認識用の入力フォーマットを取得できませんでした")
        }
        try Task.checkCancellation()
        converter = try SpeechAudioConverter(inputFormat: inputFormat, outputFormat: format)
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        self.analyzer = analyzer
        try await analyzer.prepareToAnalyze(in: format)
        try Task.checkCancellation()
        // consumer が止まっても無制限に録音を保持しない。drop は append 側で失敗として扱う。
        let (sequence, continuation) = AsyncStream<AnalyzerInput>.makeStream(bufferingPolicy: .bufferingOldest(256))
        self.continuation = continuation
        resultTask = Task { @MainActor in
            do {
                for try await result in transcriber.results {
                    guard !cancelled, !failed else { continue }
                    let transcription = SpeechTranscription(
                        text: String(result.text.characters), audioRange: result.range,
                        resultsFinalizationTime: result.resultsFinalizationTime
                    )
                    trace(transcription)
                    receive?(.result(transcription))
                }
                if !inputEnded && !cancelled && !failed {
                    throw SpeechAnalysisFailure(kind: "recognition", message: "音声認識が入力終了前に停止しました")
                }
            } catch {
                reportFailure(error)
                throw error
            }
        }
        try await analyzer.start(inputSequence: sequence)
        diagnostic("event=analysis-start sampleRate=\(format.sampleRate) channels=\(format.channelCount)")
    }

    func append(_ buffer: AVAudioPCMBuffer) throws {
        guard !finishRequested, !cancelled, !failed, let converter else {
            throw SpeechAudioConversionError.inputClosed
        }
        try converter.append(buffer, receive: yield)
    }

    private func yield(_ buffer: AVAudioPCMBuffer) throws {
        guard let continuation else { throw SpeechAudioConversionError.inputClosed }
        let start = CMTime(value: outputFrames, timescale: CMTimeScale(buffer.format.sampleRate))
        switch continuation.yield(AnalyzerInput(buffer: buffer, bufferStartTime: start)) {
        case .enqueued: outputFrames += AVAudioFramePosition(buffer.frameLength)
        case .dropped:
            throw SpeechAnalysisFailure(kind: "audio", message: "音声認識待ちの音声が上限を超えました")
        case .terminated: throw SpeechAudioConversionError.inputClosed
        @unknown default: throw SpeechAudioConversionError.inputClosed
        }
    }

    func finish() {
        guard !cancelled, !failed else { return }
        finishRequested = true
        if prepared { finishInput() }
    }

    private func finishInput() {
        guard !inputEnded, !cancelled, !failed else { return }
        do {
            guard let converter, let analyzer, let resultTask else { throw SpeechAudioConversionError.inputClosed }
            try converter.finish(receive: yield)
            inputEnded = true
            continuation?.finish()
            diagnostic("event=analysis-converted-input-ended frames=\(outputFrames) sampleRate=\(converter.outputFormat.sampleRate)")
            finishTask = Task { @MainActor in
                do {
                    try await analyzer.finalizeAndFinishThroughEndOfInput()
                    try await resultTask.value
                    // 結果をすべて受け取った後で読む。別の通知経路の時刻で未読の訂正を追い越さない。
                    let volatileRange = await analyzer.volatileRange
                    guard !cancelled, !failed else { return }
                    if let volatileRange {
                        receive?(.finalizedThrough(volatileRange.start))
                    }
                    receive?(.completed)
                } catch {
                    reportFailure(error)
                }
            }
        } catch {
            reportFailure(error)
        }
    }

    func cancel() {
        guard !cancelled else { return }
        cancelled = true
        preparationTask?.cancel()
        finishTask?.cancel()
        continuation?.finish()
        Task { @MainActor in
            await preparationTask?.value
            if let analyzer { await analyzer.cancelAndFinishNow() }
            resultTask?.cancel()
            if let resultTask {
                do { try await resultTask.value }
                catch { diagnostic("event=analysis-result-cancelled") }
            }
            await finishTask?.value
            receive?(.cancelled)
        }
    }

    private func reportFailure(_ error: Error) {
        guard !cancelled, !failed else { return }
        failed = true
        let details = error as NSError
        diagnostic("event=analysis-error domain=\(details.domain) code=\(details.code)")
        let failure: SpeechAnalysisFailure
        if let known = error as? SpeechAnalysisFailure {
            failure = known
        } else if error is SpeechAudioConversionError {
            failure = SpeechAnalysisFailure(kind: "audio", message: "音声認識用の PCM を変換・転送できませんでした")
        } else {
            failure = SpeechAnalysisFailure(kind: "recognition", message: "音声認識が正常に完了しませんでした")
        }
        receive?(.failed(failure))
    }

    private func trace(_ result: SpeechTranscription) {
        guard traceResults else { return }
        let value: [String: Any] = [
            "event": "debug-recognition", "engine": "SpeechAnalyzer", "isFinal": result.isFinal,
            "text": result.text, "audioStart": result.audioRange.start.seconds, "audioEnd": result.audioRange.end.seconds,
            "resultsFinalizationTime": result.resultsFinalizationTime.seconds,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: .sortedKeys) else { return }
        FileHandle.standardError.write(data + Data("\n".utf8))
    }
}
