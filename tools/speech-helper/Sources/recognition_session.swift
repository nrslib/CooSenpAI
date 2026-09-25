import AVFoundation
import CoreMedia
import Foundation

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
    case final(String, words: [SpeechWordTiming])
    case error(kind: String, message: String)
    case closed
}

// tap からの音声は queue が所有し、状態遷移と analyzer 操作は指定した queue に限定する。
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
    private let queue: DispatchQueue
    private let queueKey = DispatchSpecificKey<Void>()
    private var transcript: SpeechTranscript
    private var wordTimeline: [SpeechWordTiming] = []
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
        queue: DispatchQueue,
        startRecording: @escaping () -> Void,
        stopRecording: @escaping () -> Void,
        emit: @escaping (SpeechOutput) -> Void,
        diagnostic: @escaping (String) -> Void
    ) {
        self.transcript = SpeechTranscript(locale: locale)
        self.audio = audio
        self.analysis = analysis
        self.scheduler = scheduler
        self.queue = queue
        self.startRecording = startRecording
        self.stopRecording = stopRecording
        self.emit = emit
        self.diagnostic = diagnostic
        queue.setSpecific(key: queueKey, value: ())
    }

    func start(finishRequested: Bool = false) {
        performSynchronously {
            if finishRequested { self.finishOnQueue() }
            self.analysis.start { [weak self] event in self?.receive(event) }
        }
    }

    func audioAvailable() {
        performSynchronously { audioAvailableOnQueue() }
    }

    func append(_ buffer: AVAudioPCMBuffer) {
        performSynchronously {
            guard phase == .preparing || phase == .recording || phase == .finishing else { return }
            guard audio.enqueue(buffer) else { return }
            if phase == .recording || phase == .finishing { audioAvailableOnQueue() }
        }
    }

    private func audioAvailableOnQueue() {
        guard phase == .recording || phase == .finishing, !inputEnded else { return }
        if let failure = audio.failure {
            failOnQueue("audio", failure)
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
            failOnQueue("audio", "音声を認識用フォーマットへ変換・転送できませんでした")
        }
    }

    private func receive(_ event: SpeechAnalysisEvent) {
        performSynchronously { receiveOnQueue(event) }
    }

    private func receiveOnQueue(_ event: SpeechAnalysisEvent) {
        guard phase != .closed else { return }
        switch event {
        case .ready:
            if phase == .preparing {
                phase = .recording
                startRecording()
            }
            audioAvailableOnQueue()
        case let .result(result):
            guard phase == .recording || phase == .finishing else { return }
            diagnostic("event=analysis-result isFinal=\(result.isFinal) finishRequested=\(phase == .finishing) chars=\(result.text.count) audioStart=\(result.audioRange.start.seconds) audioEnd=\(result.audioRange.end.seconds) resultsFinalizationTime=\(result.resultsFinalizationTime.seconds)")
            do {
                try transcript.record(result)
                mergeWordTimings(result)
                publishPartial(transcript.text)
            } catch {
                failOnQueue("recognition", "音声認識が不正な区間の結果を返しました")
            }
        case let .finalizedThrough(time):
            guard phase == .recording || phase == .finishing else { return }
            do {
                try transcript.finalize(through: time)
                diagnostic("event=analysis-finalized-through audioTime=\(time.seconds)")
            } catch {
                failOnQueue("recognition", "音声認識が不正な確定時刻を返しました")
            }
        case .completed:
            guard phase == .finishing else {
                if phase != .closing { failOnQueue("recognition", "音声認識が入力終了前に停止しました") }
                return
            }
            guard !transcript.hasUnfinalizedText else {
                failOnQueue("recognition", "音声認識が未確定の結果を残して終了しました")
                return
            }
            complete(transcript.finalizedText)
        case let .failed(failure):
            if phase != .closing { failOnQueue(failure.kind, failure.message) }
        case .cancelled:
            if phase != .closing { failOnQueue("recognition", "音声認識が予期せず取り消されました") }
            closeOnQueue()
        }
    }

    private func publishPartial(_ text: String) {
        guard text != previousPartial else { return }
        previousPartial = text
        emit(.partial(text))
    }

    private func mergeWordTimings(_ result: SpeechTranscription) {
        guard !result.words.isEmpty else { return }
        wordTimeline.removeAll {
            CMTimeCompare($0.start, result.audioRange.end) < 0
                && CMTimeCompare(result.audioRange.start, $0.end) < 0
        }
        wordTimeline.append(contentsOf: result.words)
        wordTimeline.sort { CMTimeCompare($0.start, $1.start) < 0 }
    }

    private func complete(_ text: String) {
        guard phase == .finishing else {
            if phase != .closing { failOnQueue("recognition", "音声認識が入力終了前に停止しました") }
            return
        }
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            failOnQueue("no-speech", "音声を認識できませんでした")
            return
        }
        diagnostic("event=session-final chars=\(text.count)")
        phase = .closing
        emit(.final(text, words: wordTimeline))
        closeOnQueue()
    }

    func finish() {
        performSynchronously { finishOnQueue() }
    }

    private func finishOnQueue() {
        guard phase == .preparing || phase == .recording else { return }
        let wasPreparing = phase == .preparing
        phase = .finishing
        audio.stopAccepting()
        stopRecording()
        deadline = scheduler.schedule(after: Self.finalizationTimeout) { [weak self] in
            guard let self else { return }
            self.performSynchronously {
                guard self.phase == .finishing || self.phase == .closing else { return }
                if self.phase == .finishing {
                    self.phase = .closing
                    self.diagnostic("event=analysis-final-timeout")
                    self.emit(.error(kind: "recognition", message: "音声認識の確定処理が30秒以内に完了しませんでした"))
                    self.analysis.cancel()
                }
                self.closeOnQueue()
            }
        }
        if !wasPreparing { audioAvailableOnQueue() }
    }

    func cancel() {
        performSynchronously { cancelOnQueue() }
    }

    private func cancelOnQueue() {
        guard phase == .preparing || phase == .recording || phase == .finishing else { return }
        beginCloseOnQueue()
    }

    func fail(_ kind: String, _ message: String) {
        performSynchronously { failOnQueue(kind, message) }
    }

    private func failOnQueue(_ kind: String, _ message: String) {
        guard phase == .preparing || phase == .recording || phase == .finishing else { return }
        beginCloseOnQueue(error: .error(kind: kind, message: message))
    }

    private func beginCloseOnQueue(error: SpeechOutput? = nil) {
        let mustStop = phase != .finishing
        phase = .closing
        audio.discard()
        if mustStop {
            stopRecording()
            deadline = scheduler.schedule(after: Self.cancellationTimeout) { [weak self] in
                self?.performSynchronously { self?.closeOnQueue() }
            }
        }
        if let error { emit(error) }
        analysis.cancel()
    }

    private func closeOnQueue() {
        guard phase != .closed else { return }
        phase = .closed
        deadline?.cancel()
        deadline = nil
        audio.discard()
        emit(.closed)
    }

    private func performSynchronously(_ action: () -> Void) {
        if DispatchQueue.getSpecific(key: queueKey) != nil {
            action()
        } else {
            queue.sync(execute: action)
        }
    }
}
