import AVFoundation
import Darwin
import Foundation

enum SpeechWavDumpError: LocalizedError {
    case invalidPath, existingFile, createFailed, closed, formatMismatch, queueFull, writeFailed

    var errorDescription: String? {
        switch self {
        case .invalidPath: return "マイク音声の保存先には .wav ファイルを指定してください"
        case .existingFile: return "マイク音声の保存先に既存のファイルがあります"
        case .createFailed: return "マイク音声の WAV ファイルを作成できませんでした"
        case .closed: return "マイク音声の WAV 保存はすでに終了しています"
        case .formatMismatch: return "マイク音声の PCM 形式が録音中に変化しました"
        case .queueFull: return "マイク音声の WAV 書き込み待ちが上限を超えました"
        case .writeFailed: return "マイク音声の WAV 書き込みに失敗しました"
        }
    }
}

// tap のバッファを複製し、ディスク I/O を録音・認識のキューから分離する。
final class SpeechWavDump: @unchecked Sendable {
    private let format: AVAudioFormat
    private let queue = DispatchQueue(label: "dev.nrslib.coosenpai.speech.wav-dump", qos: .utility)
    private let lock = NSLock()
    private var closed = false
    private var pendingBufferCount = 0
    private var failure: SpeechWavDumpError?
    private let file: SpeechWavFile
    private var writtenFrames: AVAudioFramePosition = 0

    init(path: String, format: AVAudioFormat) throws {
        let url = URL(fileURLWithPath: path)
        guard !path.isEmpty, url.pathExtension.lowercased() == "wav" else {
            throw SpeechWavDumpError.invalidPath
        }
        let descriptor = open(url.path, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        guard descriptor >= 0 else {
            throw errno == EEXIST ? SpeechWavDumpError.existingFile : SpeechWavDumpError.createFailed
        }
        self.format = format
        file = try SpeechWavFile(descriptor: descriptor, format: format)
    }

    func append(_ buffer: AVAudioPCMBuffer) throws {
        lock.lock()
        defer { lock.unlock() }
        guard !closed else { throw SpeechWavDumpError.closed }
        if let failure { throw failure }
        guard buffer.frameLength > 0 else { return }
        guard buffer.format.sampleRate == format.sampleRate,
              buffer.format.channelCount == format.channelCount,
              buffer.format.commonFormat == format.commonFormat,
              buffer.format.isInterleaved == format.isInterleaved else {
            throw SpeechWavDumpError.formatMismatch
        }
        guard pendingBufferCount < 256 else { throw SpeechWavDumpError.queueFull }
        let copied = try deepCopyAudioBuffer(buffer)
        pendingBufferCount += 1
        queue.async { self.write(copied) }
    }

    private func write(_ buffer: AVAudioPCMBuffer) {
        defer {
            lock.lock()
            pendingBufferCount -= 1
            lock.unlock()
        }
        lock.lock()
        let failed = failure != nil
        lock.unlock()
        guard !failed else { return }
        do {
            try file.write(buffer)
            writtenFrames += AVAudioFramePosition(buffer.frameLength)
        } catch {
            lock.lock()
            failure = .writeFailed
            lock.unlock()
        }
    }

    func close() throws -> AVAudioFramePosition {
        lock.lock()
        closed = true
        lock.unlock()
        return try queue.sync {
            do { try file.close() }
            catch {
                lock.lock()
                failure = .writeFailed
                lock.unlock()
            }
            lock.lock()
            defer { lock.unlock() }
            if let failure { throw failure }
            return writtenFrames
        }
    }
}
