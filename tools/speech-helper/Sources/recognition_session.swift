import AVFoundation
import CoreMedia
import Foundation

struct SpeechAnalysisFailure: Error {
    let kind: String
    let message: String
}

enum SpeechAnalysisEvent {
    case ready
    case result(SpeechTranscription)
    case partialTranscript(String)
    case completedTranscript(String)
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

protocol SpeechDeadline: AnyObject {
    func cancel()
}

protocol SpeechDeadlineScheduler {
    func schedule(after seconds: TimeInterval, action: @escaping () -> Void) -> SpeechDeadline
}

extension DispatchWorkItem: SpeechDeadline {}

struct MainQueueSpeechDeadlineScheduler: SpeechDeadlineScheduler {
    func schedule(after seconds: TimeInterval, action: @escaping () -> Void) -> SpeechDeadline {
        let item = DispatchWorkItem(block: action)
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: item)
        return item
    }
}

enum SpeechOutput: Equatable {
    case partial(String)
    case final(String)
    case error(kind: String, message: String)
    case closed
}

// tap からの音声は queue が所有し、状態遷移と analyzer 操作は main queue に限定する。
final class SpeechRecognitionSession: @unchecked Sendable {
    static let finalizationTimeout: TimeInterval = 30
    static let cancellationTimeout: TimeInterval = 30

    private enum Phase { case preparing, recording, finishing, closing, closed }

    private let audio: SpeechAudioQueue
    private let analysis: SpeechAnalysis
    private let scheduler: SpeechDeadlineScheduler
    private let startRecording: () -> Void
    private let stopRecording: () -> Void
    private let emit: (SpeechOutput) -> Void
    private let diagnostic: (String) -> Void
    private var transcript: SpeechTranscript
    private var phase = Phase.preparing
    private var deadline: SpeechDeadline?
    private var inputEnded = false
    private var appendedFrames: AVAudioFramePosition = 0
    private var previousPartial: String?

    init(
        locale: Locale,
        audio: SpeechAudioQueue,
        analysis: SpeechAnalysis,
        scheduler: SpeechDeadlineScheduler,
        startRecording: @escaping () -> Void,
        stopRecording: @escaping () -> Void,
        emit: @escaping (SpeechOutput) -> Void,
        diagnostic: @escaping (String) -> Void
    ) {
        self.transcript = SpeechTranscript(locale: locale)
        self.audio = audio
        self.analysis = analysis
        self.scheduler = scheduler
        self.startRecording = startRecording
        self.stopRecording = stopRecording
        self.emit = emit
        self.diagnostic = diagnostic
    }

    func start(finishRequested: Bool = false) {
        if finishRequested { finish() }
        analysis.start { [weak self] event in self?.receive(event) }
    }

    func audioAvailable() {
        guard phase == .recording || phase == .finishing, !inputEnded else { return }
        if let failure = audio.failure {
            fail("audio", failure)
            return
        }
        do {
            for buffer in audio.takeAll() {
                try analysis.append(buffer)
                appendedFrames += AVAudioFramePosition(buffer.frameLength)
            }
            if phase == .finishing {
                inputEnded = true
                diagnostic("event=analysis-input-ended frames=\(appendedFrames)")
                analysis.finish()
            }
        } catch {
            fail("audio", "音声を認識用フォーマットへ変換・転送できませんでした")
        }
    }

    private func receive(_ event: SpeechAnalysisEvent) {
        guard phase != .closed else { return }
        switch event {
        case .ready:
            if phase == .preparing {
                phase = .recording
                startRecording()
            } else if phase == .finishing {
                audioAvailable()
            }
        case let .result(result):
            guard phase == .recording || phase == .finishing else { return }
            diagnostic("event=analysis-result isFinal=\(result.isFinal) finishRequested=\(phase == .finishing) chars=\(result.text.count) audioStart=\(result.audioRange.start.seconds) audioEnd=\(result.audioRange.end.seconds) resultsFinalizationTime=\(result.resultsFinalizationTime.seconds)")
            do {
                try transcript.record(result)
                publishPartial(transcript.text)
            } catch {
                fail("recognition", "音声認識が不正な区間の結果を返しました")
            }
        case let .finalizedThrough(time):
            guard phase == .recording || phase == .finishing else { return }
            do {
                try transcript.finalize(through: time)
                diagnostic("event=analysis-finalized-through audioTime=\(time.seconds)")
            } catch {
                fail("recognition", "音声認識が不正な確定時刻を返しました")
            }
        case let .partialTranscript(text):
            guard phase == .recording || phase == .finishing else { return }
            publishPartial(text)
        case let .completedTranscript(text):
            complete(text)
        case .completed:
            guard phase == .finishing else {
                if phase != .closing { fail("recognition", "音声認識が入力終了前に停止しました") }
                return
            }
            guard !transcript.hasUnfinalizedText else {
                fail("recognition", "音声認識が未確定の結果を残して終了しました")
                return
            }
            complete(transcript.finalizedText)
        case let .failed(failure):
            if phase != .closing { fail(failure.kind, failure.message) }
        case .cancelled:
            if phase != .closing { fail("recognition", "音声認識が予期せず取り消されました") }
            close()
        }
    }

    private func publishPartial(_ text: String) {
        guard text != previousPartial else { return }
        previousPartial = text
        emit(.partial(text))
    }

    private func complete(_ text: String) {
        guard phase == .finishing else {
            if phase != .closing { fail("recognition", "音声認識が入力終了前に停止しました") }
            return
        }
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            fail("no-speech", "音声を認識できませんでした")
            return
        }
        diagnostic("event=session-final chars=\(text.count)")
        phase = .closing
        emit(.final(text))
        close()
    }

    func finish() {
        guard phase == .preparing || phase == .recording else { return }
        let wasPreparing = phase == .preparing
        phase = .finishing
        audio.stopAccepting()
        stopRecording()
        deadline = scheduler.schedule(after: Self.finalizationTimeout) { [weak self] in
            guard let self, self.phase == .finishing || self.phase == .closing else { return }
            if self.phase == .finishing {
                self.phase = .closing
                self.diagnostic("event=analysis-final-timeout")
                self.emit(.error(kind: "recognition", message: "音声認識の確定処理が30秒以内に完了しませんでした"))
                self.analysis.cancel()
            }
            self.close()
        }
        if !wasPreparing { audioAvailable() }
    }

    func cancel() {
        guard phase == .preparing || phase == .recording || phase == .finishing else { return }
        beginClose()
    }

    func fail(_ kind: String, _ message: String) {
        guard phase == .preparing || phase == .recording || phase == .finishing else { return }
        beginClose(error: .error(kind: kind, message: message))
    }

    private func beginClose(error: SpeechOutput? = nil) {
        let mustStop = phase != .finishing
        phase = .closing
        audio.discard()
        if mustStop {
            stopRecording()
            deadline = scheduler.schedule(after: Self.cancellationTimeout) { [weak self] in self?.close() }
        }
        if let error { emit(error) }
        analysis.cancel()
    }

    private func close() {
        guard phase != .closed else { return }
        phase = .closed
        deadline?.cancel()
        deadline = nil
        audio.discard()
        emit(.closed)
    }
}
