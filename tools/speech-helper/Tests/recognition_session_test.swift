import AVFoundation
import CoreMedia
import Foundation

private final class TestDeadline: SpeechDeadline {
    let time: TimeInterval
    let action: () -> Void
    var cancelled = false
    var fired = false

    init(time: TimeInterval, action: @escaping () -> Void) {
        self.time = time
        self.action = action
    }

    func cancel() { cancelled = true }
}

private final class TestScheduler: SpeechDeadlineScheduler {
    private var now: TimeInterval = 0
    private(set) var deadlines: [TestDeadline] = []

    func schedule(after seconds: TimeInterval, action: @escaping () -> Void) -> SpeechDeadline {
        let deadline = TestDeadline(time: now + seconds, action: action)
        deadlines.append(deadline)
        return deadline
    }

    func advance(by seconds: TimeInterval) {
        let target = now + seconds
        while let next = deadlines.filter({ !$0.cancelled && !$0.fired && $0.time <= target })
            .min(by: { $0.time < $1.time }) {
            now = next.time
            next.fired = true
            next.action()
        }
        now = target
    }
}

private final class TestAnalysis: SpeechAnalysis {
    private var receive: ((SpeechAnalysisEvent) -> Void)?
    private(set) var startCount = 0
    private(set) var finishCount = 0
    private(set) var cancelCount = 0
    private(set) var samples: [Float] = []
    var failAppend = false
    var synchronousCancel = false
    var synchronousStartEvent: SpeechAnalysisEvent?

    func start(receive: @escaping (SpeechAnalysisEvent) -> Void) {
        startCount += 1
        self.receive = receive
        if let synchronousStartEvent { receive(synchronousStartEvent) }
    }

    func append(_ buffer: AVAudioPCMBuffer) throws {
        expect(finishCount == 0 && cancelCount == 0, "終了済み analyzer に音声を渡さない")
        if failAppend { throw SpeechAudioConversionError.conversionFailed }
        samples.append(buffer.floatChannelData![0][0])
    }

    func finish() {
        finishCount += 1
        expect(finishCount == 1, "入力終了は一度だけ")
    }

    func cancel() {
        cancelCount += 1
        expect(cancelCount == 1, "取消は一度だけ")
        if synchronousCancel { send(.cancelled) }
    }

    func send(_ event: SpeechAnalysisEvent) { receive?(event) }
    func result(_ text: String, start: Int64 = 0, end: Int64 = 48_000, isFinal: Bool = false) {
        send(.result(speechTranscription(text, start: start, end: end, isFinal: isFinal)))
    }
}

private final class RecordingOutput {
    var events: [SpeechOutput] = []
    var diagnostics: [String] = []
    var startCount = 0
    var stopCount = 0
    var onEmit: ((SpeechOutput) -> Void)?

    var finals: [String] {
        events.compactMap { if case let .final(text) = $0 { return text }; return nil }
    }
    var errors: [String] {
        events.compactMap { if case let .error(kind, _) = $0 { return kind }; return nil }
    }
    var closedCount: Int { events.filter { $0 == .closed }.count }
}

private final class SessionHarness {
    let audio = SpeechAudioQueue()
    let analysis = TestAnalysis()
    let scheduler = TestScheduler()
    let output = RecordingOutput()
    let session: SpeechRecognitionSession

    init(
        finishRequested: Bool = false,
        synchronousStartEvent: SpeechAnalysisEvent? = nil,
        synchronousCancel: Bool = false
    ) {
        let output = self.output
        analysis.synchronousStartEvent = synchronousStartEvent
        analysis.synchronousCancel = synchronousCancel
        session = SpeechRecognitionSession(
            locale: Locale(identifier: "ja-JP"), audio: audio, analysis: analysis, scheduler: scheduler,
            startRecording: { output.startCount += 1 },
            stopRecording: { output.stopCount += 1 },
            emit: { output.events.append($0); output.onEmit?($0) },
            diagnostic: { output.diagnostics.append($0) }
        )
        session.start(finishRequested: finishRequested)
    }

    func ready() { analysis.send(.ready) }
    func append(_ sample: Float) {
        if audio.enqueue(makeBuffer(sample)) { session.audioAvailable() }
    }
}

private func makeBuffer(_ sample: Float) -> AVAudioPCMBuffer {
    let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
    buffer.frameLength = 1
    buffer.floatChannelData![0][0] = sample
    return buffer
}

func testRecognitionSession() {
    runTest("起動前の finish を同期 ready より先に反映し、録音・WAV を開始しない") {
        for ending in ["complete", "cancel", "error"] {
            let h = SessionHarness(finishRequested: true, synchronousStartEvent: .ready)
            expect(h.output.startCount == 0 && h.output.stopCount == 1, "ready を出す録音開始処理へ進まない")
            expect(h.analysis.startCount == 1 && h.analysis.finishCount == 1, "同期 ready でも入力を一度だけ閉じる")
            h.append(1)
            expect(h.analysis.samples.isEmpty, "finish 後の PCM を渡さない")
            h.analysis.synchronousCancel = true
            switch ending {
            case "complete": h.analysis.send(.completedTranscript(""))
            case "cancel": h.session.cancel()
            default: h.analysis.send(.failed(SpeechAnalysisFailure(kind: "recognition", message: "認識失敗")))
            }
            h.ready()
            h.analysis.send(.completedTranscript("遅い結果"))
            expect(h.output.startCount == 0 && h.output.stopCount == 1, "終了後も録音を開始せず二重停止しない")
            expect(h.output.closedCount == 1 && h.output.finals.isEmpty, "停止・取消・エラーを一度だけ処理する")
            expect(h.output.errors == (ending == "cancel" ? [] : [ending == "complete" ? "no-speech" : "recognition"]), "終了理由を保持する")
        }
    }

    runTest("起動前に finish 済みなら同期準備エラーの後も録音しない") {
        let h = SessionHarness(finishRequested: true, synchronousStartEvent: .failed(
            SpeechAnalysisFailure(kind: "permission-speech", message: "認識権限なし")
        ), synchronousCancel: true)
        expect(h.output.closedCount == 1, "同期的な準備エラーと取消完了を起動中に処理する")
        h.analysis.send(.cancelled)
        h.ready()
        expect(h.output.startCount == 0 && h.analysis.samples.isEmpty, "失敗後の ready で入力を開始しない")
        expect(h.output.errors == ["permission-speech"] && h.output.closedCount == 1, "準備エラーを保持する")
    }

    runTest("SFSpeechRecognizer の全文訂正を反映し、finish 後の確定通知だけを採用する") {
        let h = SessionHarness()
        h.ready()
        h.append(1)
        h.analysis.send(.partialTranscript("最初の文"))
        h.analysis.send(.partialTranscript("続きの文"))
        h.analysis.send(.partialTranscript("続きの文"))
        expect(h.output.events == [.partial("最初の文"), .partial("続きの文")], "古い全文を連結・再送しない")
        expect(h.audio.enqueue(makeBuffer(2)), "未処理の PCM を保持する")
        h.session.finish()
        h.scheduler.advance(by: 3)
        expect(h.output.finals.isEmpty, "固定時間で部分結果を確定しない")
        expect(h.analysis.samples == [1, 2] && h.analysis.finishCount == 1, "終了前に全 PCM を渡す")
        h.analysis.send(.completedTranscript("続きの確定文"))
        h.analysis.send(.completedTranscript("遅い結果"))
        expect(h.output.finals == ["続きの確定文"] && h.output.closedCount == 1, "確定は一度だけ")
    }

    runTest("SFSpeechRecognizer の早期終了・空結果・取消後の結果を送信しない") {
        for scenario in ["early", "empty", "cancel"] {
            let h = SessionHarness()
            h.analysis.synchronousCancel = true
            h.ready()
            h.analysis.send(.partialTranscript("途中の文"))
            if scenario != "early" { h.session.finish() }
            if scenario == "cancel" { h.session.cancel() }
            h.analysis.send(.completedTranscript(scenario == "empty" ? "" : "確定文"))
            expect(h.output.finals.isEmpty && h.output.closedCount == 1, "失敗と取消で本文を返さない")
            expect(h.output.errors == (scenario == "cancel" ? [] : [scenario == "empty" ? "no-speech" : "recognition"]), "終端の原因を保持する")
        }
    }

    runTest("モデル・analyzer の準備完了まで録音を開始しない") {
        let h = SessionHarness()
        expect(h.output.startCount == 0, "準備中は未録音")
        h.ready()
        h.ready()
        expect(h.output.startCount == 1 && h.analysis.startCount == 1, "録音と analyzer は一度だけ開始する")
        h.session.cancel()
        h.analysis.send(.cancelled)
    }

    runTest("2発話・無音・finish 直前の PCM を同じ analyzer に渡し、結果完了後に一度だけ全文確定する") {
        let h = SessionHarness()
        h.ready()
        h.append(1)
        h.analysis.result("最初の文をここで話しますま")
        h.analysis.result("最初の文をここで話します", isFinal: true)
        h.scheduler.advance(by: 3.5)
        h.append(2)
        h.analysis.result("そして続きの文をここで", start: 216_000, end: 360_000)
        expect(h.analysis.startCount == 1 && h.analysis.finishCount == 0, "無音と区間確定で analyzer を再開・終了しない")
        expect(h.audio.enqueue(makeBuffer(3)), "finish 直前の未処理音声を保持する")
        h.session.finish()
        h.session.finish()
        h.session.audioAvailable()
        expect(h.analysis.samples == [1, 2, 3] && h.analysis.finishCount == 1, "全受付済み PCM を入力終了前に渡す")
        expect(h.output.diagnostics.contains("event=analysis-input-ended frames=3"), "実際に append したフレーム数を記録する")
        expect(h.output.stopCount == 1 && h.output.finals.isEmpty, "録音を止めても即時確定しない")
        h.analysis.result("そして続きの文をここで話しますま", start: 216_000, end: 360_000)
        h.analysis.result("そして続きの文をここで話します", start: 216_000, end: 360_000, isFinal: true)
        h.scheduler.advance(by: 2)
        expect(h.output.finals.isEmpty, "isFinal や固定時間では結果列を打ち切らない")
        h.analysis.send(.completed)
        expect(h.output.finals == ["最初の文をここで話しますそして続きの文をここで話します"], "確定結果だけから全文を返す")
        expect(h.output.closedCount == 1 && h.analysis.cancelCount == 0, "正常完了後は取消しない")
        h.analysis.send(.completed)
        h.analysis.send(.failed(SpeechAnalysisFailure(kind: "recognition", message: "遅い通知")))
        h.analysis.result("遅い本文", isFinal: true)
        h.session.finish()
        h.session.cancel()
        h.scheduler.advance(by: 60)
        expect(h.output.finals.count == 1 && h.output.errors.isEmpty && h.output.closedCount == 1, "終端後の通知を無視する")
    }

    runTest("同じ全文の partial 通知を重複送信しない") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("本文")
        h.analysis.result("本文")
        h.analysis.result("本文", isFinal: true)
        expect(h.output.events == [.partial("本文")], "区間確定でも表示本文が同じなら再送しない")
        h.session.cancel()
        h.analysis.send(.cancelled)
    }

    runTest("本文の再通知なしで volatileRange の確定時刻から全文を確定する") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("最初の文")
        h.analysis.result("続きの文", start: 96_000, end: 144_000)
        h.analysis.result("最初の文の訂正")
        expect(h.output.events.last == .partial("最初の文の訂正続きの文"), "複数の未確定区間を全文表示する")
        h.session.finish()
        let before = h.output.events
        h.analysis.send(.finalizedThrough(CMTime(value: 144_000, timescale: 48_000)))
        expect(h.output.events == before, "表示本文を重複せず完了通知を待つ")
        h.analysis.send(.completed)
        h.analysis.send(.completed)
        h.analysis.send(.finalizedThrough(CMTime(value: 4, timescale: 1)))
        expect(h.output.finals == ["最初の文の訂正続きの文"], "isFinal の再発行がなくても一度だけ全文を返す")
        expect(h.output.errors.isEmpty && h.output.closedCount == 1, "正常に終了する")
    }

    runTest("確定範囲が不足した完了は成功に代替しない") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("まだ仮説")
        h.session.finish()
        h.analysis.send(.finalizedThrough(CMTime(value: 1, timescale: 2)))
        h.analysis.send(.completed)
        expect(h.output.errors == ["recognition"] && h.output.finals.isEmpty, "volatile だけを final にしない")
        h.analysis.send(.cancelled)
        expect(h.output.closedCount == 1, "取消完了後に閉じる")
    }

    runTest("取消後の確定時刻から本文を送らない") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("途中の本文")
        h.session.finish()
        h.session.cancel()
        h.analysis.send(.finalizedThrough(CMTime(value: 1, timescale: 1)))
        h.analysis.send(.completed)
        h.analysis.send(.cancelled)
        expect(h.output.finals.isEmpty && h.output.errors.isEmpty && h.output.closedCount == 1, "取消が確定より優先する")
    }

    runTest("無音の完了は no-speech を返す") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("誤った仮説")
        h.analysis.result("", isFinal: true)
        h.session.finish()
        h.analysis.send(.completed)
        h.analysis.send(.cancelled)
        expect(h.output.errors == ["no-speech"] && h.output.finals.isEmpty && h.output.closedCount == 1, "空結果を以前の仮説で補わない")
    }

    runTest("準備中の finish は録音を開始せず、準備時間を含む30秒で閉じる") {
        let h = SessionHarness()
        h.session.finish()
        h.scheduler.advance(by: 20)
        h.ready()
        expect(h.output.startCount == 0 && h.analysis.finishCount == 1, "準備後もマイク・WAV を開始しない")
        h.scheduler.advance(by: 9)
        expect(h.output.errors.isEmpty && h.output.closedCount == 0, "期限前は完了を待つ")
        h.scheduler.advance(by: 1)
        expect(h.output.errors == ["recognition"] && h.output.closedCount == 1 && h.analysis.cancelCount == 1, "準備完了で期限を延長しない")
    }

    runTest("準備が完了しない finish も30秒で閉じる") {
        let h = SessionHarness()
        h.session.finish()
        h.scheduler.advance(by: 30)
        h.ready()
        expect(h.output.errors == ["recognition"] && h.output.closedCount == 1 && h.output.startCount == 0, "期限後に録音を始めない")
    }

    runTest("final 期限の取消が同期的に通知されても error と closed は一度だけ") {
        let h = SessionHarness()
        h.ready()
        h.analysis.synchronousCancel = true
        h.session.finish()
        h.scheduler.advance(by: 30)
        expect(h.output.errors == ["recognition"] && h.output.closedCount == 1 && h.output.finals.isEmpty, "timeout の終端を先に獲得する")
    }

    runTest("finish 後のエラーや取消で期限を再設定しない") {
        for isError in [false, true] {
            let h = SessionHarness()
            h.ready()
            h.session.finish()
            h.scheduler.advance(by: 29)
            if isError { h.session.fail("audio", "転送失敗") }
            else { h.session.cancel() }
            h.scheduler.advance(by: 1)
            expect(h.output.closedCount == 1 && h.output.finals.isEmpty, "元の finish から30秒で閉じる")
            expect(h.output.errors == (isError ? ["audio"] : []), "取消を認識失敗へ変更しない")
            expect(h.analysis.cancelCount == 1 && h.output.stopCount == 1, "二重取消・二重録音停止をしない")
        }
    }

    runTest("準備中と録音中の cancel は完了通知を待ち、本文を返さない") {
        for prepared in [false, true] {
            let h = SessionHarness()
            if prepared { h.ready(); h.append(1); h.analysis.result("途中") }
            h.session.cancel()
            h.session.cancel()
            h.ready()
            h.analysis.result("遅い確定", isFinal: true)
            h.analysis.send(.completed)
            expect(h.output.closedCount == 0 && h.analysis.cancelCount == 1, "取消処理の完了を待つ")
            h.analysis.send(.cancelled)
            h.scheduler.advance(by: 60)
            expect(h.output.finals.isEmpty && h.output.errors.isEmpty && h.output.closedCount == 1, "取消では本文を返さない")
        }
    }

    runTest("cancel の完了通知が来ない場合は30秒で閉じる") {
        let h = SessionHarness()
        h.session.cancel()
        h.scheduler.advance(by: 29)
        expect(h.output.closedCount == 0, "期限前に process を閉じない")
        h.scheduler.advance(by: 1)
        expect(h.output.closedCount == 1 && h.output.errors.isEmpty, "取消期限で閉じる")
    }

    runTest("入力終了前の結果列停止を成功として扱わない") {
        for event in [SpeechAnalysisEvent.completed, .cancelled] {
            let h = SessionHarness()
            h.ready()
            h.analysis.result("確定区間", isFinal: true)
            h.analysis.send(event)
            h.analysis.send(.cancelled)
            expect(h.output.errors == ["recognition"] && h.output.finals.isEmpty && h.output.closedCount == 1, "予期しない完了は失敗にする")
        }
    }

    runTest("モデル準備の失敗では録音せず、元の種別を返す") {
        let h = SessionHarness()
        h.analysis.send(.failed(SpeechAnalysisFailure(kind: "on-device-unsupported", message: "利用不可")))
        h.ready()
        h.analysis.send(.cancelled)
        expect(h.output.errors == ["on-device-unsupported"] && h.output.startCount == 0 && h.output.closedCount == 1, "別の認識器へ代替しない")
    }

    runTest("変換・転送失敗とキュー上限は fail closed にする") {
        for overflow in [false, true] {
            let h = SessionHarness()
            h.ready()
            if overflow {
                for _ in 0...SpeechAudioQueue.maximumBufferCount { _ = h.audio.enqueue(makeBuffer(1)) }
            } else {
                h.analysis.failAppend = true
                _ = h.audio.enqueue(makeBuffer(1))
            }
            h.session.audioAvailable()
            h.analysis.send(.cancelled)
            expect(h.output.errors == ["audio"] && h.output.finals.isEmpty && h.output.closedCount == 1, "欠けた音声で成功にしない")
            expect(h.audio.takeAll().isEmpty && h.analysis.samples.isEmpty, "失敗後は音声を破棄する")
        }
    }

    runTest("キューが tap のコピーを所有し、finish 後は追加を拒否する") {
        let h = SessionHarness()
        h.ready()
        let buffer = makeBuffer(1)
        expect(h.audio.enqueue(buffer), "最初の通知を要求する")
        buffer.floatChannelData![0][0] = 2
        expect(!h.audio.enqueue(buffer), "main queue 通知をバッファごとに増やさない")
        h.session.finish()
        expect(h.analysis.samples == [1, 2], "再利用された元バッファに影響されない")
        expect(!h.audio.enqueue(makeBuffer(3)), "終了後に音声を受け付けない")
        h.session.cancel()
        h.analysis.send(.cancelled)
    }

    runTest("不正な確定範囲を失敗として返す") {
        let h = SessionHarness()
        h.ready()
        h.analysis.result("前半", isFinal: true)
        h.analysis.result("重なり", start: 47_999, end: 96_000, isFinal: true)
        h.analysis.send(.cancelled)
        expect(h.output.errors == ["recognition"] && h.output.finals.isEmpty, "壊れた区間を推測でつながない")
    }

    runTest("出力先から再入しても終端を重複させない") {
        for success in [false, true] {
            let h = SessionHarness()
            h.ready()
            h.analysis.synchronousCancel = true
            h.output.onEmit = { [weak h] event in
                guard let h else { return }
                if case .closed = event { return }
                h.session.cancel()
                h.session.fail("recognition", "再入")
            }
            if success {
                h.output.onEmit = nil
                h.analysis.result("本文", isFinal: true)
                h.session.finish()
                h.output.onEmit = { [weak h] _ in h?.session.cancel(); h?.session.fail("recognition", "再入") }
                h.analysis.send(.completed)
            } else {
                h.session.fail("audio", "失敗")
            }
            expect(h.output.closedCount == 1, "closed は一度だけ")
            expect(h.output.errors == (success ? [] : ["audio"]), "元の終端だけを通知する")
            expect(h.output.finals == (success ? ["本文"] : []), "成功時だけ final を返す")
        }
    }
}
