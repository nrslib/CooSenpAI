import AVFoundation
import Foundation

enum DebugInputWavError: LocalizedError {
    case emptyPath
    case openFailed(String)
    case unsupportedFormat
    case emptyFile
    case invalidPlaybackRate
    case bufferAllocation
    case readFailed(String)
    case invalidBufferDuration

    var errorDescription: String? {
        switch self {
        case .emptyPath:
            return "デバッグ用 WAV のパスが空です"
        case let .openFailed(details):
            return "デバッグ用 WAV を開けませんでした: \(details)"
        case .unsupportedFormat:
            return "デバッグ用 WAV の PCM フォーマットを利用できません"
        case .emptyFile:
            return "デバッグ用 WAV に音声フレームがありません"
        case .invalidPlaybackRate:
            return "デバッグ用 WAV の再生速度が不正です"
        case .bufferAllocation:
            return "デバッグ用 WAV の音声バッファを確保できません"
        case let .readFailed(details):
            return "デバッグ用 WAV を読み込めませんでした: \(details)"
        case .invalidBufferDuration:
            return "デバッグ用 WAV のバッファ時間を計算できません"
        }
    }
}

final class DebugInputWavPlayer {
    let source: AudioSource = .microphone
    let format: AVAudioFormat
    let frameLength: AVAudioFramePosition

    private let audioFile: AVAudioFile
    private let playbackRate: Double
    private let bufferFrameCapacity: AVAudioFrameCount = 1_024
    private let queue = DispatchQueue(
        label: "dev.nrslib.coosenpai.hearing.debug-input",
        qos: .userInitiated
    )
    private var timer: DispatchSourceTimer?
    private var started = false
    private var stopped = true
    private var bufferHandler: ((AVAudioPCMBuffer) -> Void)?
    private var completionHandler: ((Result<Void, Error>) -> Void)?

    init(path: String, playbackRate: Double) throws {
        guard !path.isEmpty else { throw DebugInputWavError.emptyPath }
        guard playbackRate.isFinite, playbackRate > 0 else {
            throw DebugInputWavError.invalidPlaybackRate
        }
        do {
            audioFile = try AVAudioFile(forReading: URL(fileURLWithPath: path))
        } catch {
            throw DebugInputWavError.openFailed(String(describing: error))
        }
        format = audioFile.processingFormat
        frameLength = audioFile.length
        guard format.sampleRate.isFinite,
              format.sampleRate > 0,
              format.channelCount > 0 else {
            throw DebugInputWavError.unsupportedFormat
        }
        guard frameLength > 0 else { throw DebugInputWavError.emptyFile }
        self.playbackRate = playbackRate
    }

    func start(
        onBuffer: @escaping (AVAudioPCMBuffer) -> Void,
        onCompletion: @escaping (Result<Void, Error>) -> Void
    ) {
        queue.async { [weak self] in
            guard let self, !self.started else { return }
            self.started = true
            self.stopped = false
            self.bufferHandler = onBuffer
            self.completionHandler = onCompletion
            let timer = DispatchSource.makeTimerSource(queue: self.queue)
            timer.setEventHandler { [weak self] in
                self?.emitNextBuffer()
            }
            self.timer = timer
            timer.schedule(deadline: .now())
            timer.resume()
        }
    }

    func stop() {
        queue.async { [weak self] in
            self?.stopOnQueue()
        }
    }

    private func emitNextBuffer() {
        guard !stopped else { return }
        do {
            guard let buffer = try readNextBuffer() else {
                finish(.success(()))
                return
            }
            bufferHandler?(buffer)
            scheduleNextBuffer(after: buffer)
        } catch {
            finish(.failure(error))
        }
    }

    private func readNextBuffer() throws -> AVAudioPCMBuffer? {
        guard audioFile.framePosition < audioFile.length else { return nil }
        let remainingFrames = audioFile.length - audioFile.framePosition
        let frameCount = AVAudioFrameCount(
            min(remainingFrames, AVAudioFramePosition(bufferFrameCapacity))
        )
        guard frameCount > 0,
              let buffer = AVAudioPCMBuffer(
                  pcmFormat: audioFile.processingFormat,
                  frameCapacity: frameCount
              ) else {
            throw DebugInputWavError.bufferAllocation
        }
        do {
            try audioFile.read(into: buffer, frameCount: frameCount)
        } catch {
            throw DebugInputWavError.readFailed(String(describing: error))
        }
        guard buffer.frameLength > 0 else { return nil }
        return buffer
    }

    private func scheduleNextBuffer(after buffer: AVAudioPCMBuffer) {
        let duration = Double(buffer.frameLength) / format.sampleRate / playbackRate
        guard duration.isFinite, duration > 0,
              duration <= Double(UInt64.max) / 1_000_000_000 else {
            finish(.failure(DebugInputWavError.invalidBufferDuration))
            return
        }
        let delayNanoseconds = UInt64(ceil(duration * 1_000_000_000))
        let delay = Int(min(max(delayNanoseconds, 1), UInt64(Int.max)))
        timer?.schedule(deadline: .now() + .nanoseconds(delay))
    }

    private func finish(_ result: Result<Void, Error>) {
        guard !stopped else { return }
        stopped = true
        timer?.cancel()
        timer = nil
        let completion = completionHandler
        completionHandler = nil
        bufferHandler = nil
        completion?(result)
    }

    private func stopOnQueue() {
        guard !stopped else { return }
        stopped = true
        timer?.cancel()
        timer = nil
        completionHandler = nil
        bufferHandler = nil
    }
}
