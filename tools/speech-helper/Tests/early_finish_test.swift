import AVFoundation
import Foundation

private final class EarlyFinishProbe {
    private let engine: SpeechEngine
    private let mode: String
    private let audio = SpeechAudioQueue()
    private lazy var analysis: SpeechAnalysis = engine.makeAnalysis(locale: Locale(identifier: "ja-JP"),
        inputFormat: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!,
        traceResults: false, diagnostic: { [weak self] message in
            self?.diagnostics.append(message)
            print(message)
        })
    private var session: SpeechRecognitionSession?
    private var diagnostics: [String] = []
    private var recordingStarts = 0
    private var readyCount = 0
    private let startedAt = Date()

    init(engine: SpeechEngine, mode: String) {
        self.engine = engine
        self.mode = mode
    }

    func start() {
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) {
            expect(false, "実エンジンの先行 finish が10秒以内に完了しない")
        }
        if mode == "session" {
            session = SpeechRecognitionSession(locale: Locale(identifier: "ja-JP"), audio: audio,
                analysis: analysis, scheduler: MainQueueSpeechDeadlineScheduler(),
                startRecording: { [self] in recordingStarts += 1 }, stopRecording: {},
                emit: { [self] output in
                    switch output {
                    case .closed: complete()
                    case let .error(kind, _): expect(kind == "no-speech", "音声のない正常終了: \(kind)")
                    default: expect(false, "先行 finish で音声認識結果を出さない")
                    }
                }, diagnostic: { [self] in diagnostics.append($0) })
            session!.start(finishRequested: true)
        } else {
            if mode == "before-start" { analysis.finish(); analysis.finish() }
            analysis.start { [self] event in
                switch event {
                case .ready:
                    readyCount += 1
                    if mode == "during-start" { analysis.finish(); analysis.finish() }
                case .completed: complete()
                case let .completedTranscript(text):
                    expect(text.isEmpty, "音声を投入していない")
                    complete()
                case let .failed(failure): expect(false, "先行 finish が失敗: \(failure.kind)")
                default: expect(false, "先行 finish で予期しない認識イベント")
                }
            }
            if mode == "during-start" { analysis.finish() }
        }
    }

    private func complete() {
        expect(recordingStarts == 0, "マイク・WAV を開始しない")
        expect(mode == "session" ? readyCount == 0 : readyCount == 1, "ready の通知回数")
        let ending = engine == .sf ? "event=recognizer-input-ended engine=SFSpeechRecognizer"
            : "event=analysis-converted-input-ended frames=0 "
        expect(diagnostics.filter { $0.hasPrefix(ending) }.count == 1, "実エンジンの入力終了を一度だけ適用する")
        expect(!diagnostics.contains("event=analysis-final-timeout"), "最終化期限で終了しない")
        print("PASS early-finish engine=\(engine.name) mode=\(mode) elapsed=\(Date().timeIntervalSince(startedAt))")
        exit(0)
    }
}

func testRealEarlyFinish(engine: SpeechEngine, mode: String) -> Never {
    let probe = EarlyFinishProbe(engine: engine, mode: mode)
    return withExtendedLifetime(probe) { () -> Never in
        probe.start()
        dispatchMain()
    }
}
