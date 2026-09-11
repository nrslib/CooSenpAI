import AVFoundation
import Foundation

// AVAudioEngine は tap の buffer を再利用するため、queue が音声のコピーを所有する。
final class SpeechAudioQueue: @unchecked Sendable {
    static let maximumBufferCount = 256
    private let lock = NSLock()
    private var buffers: [AVAudioPCMBuffer] = []
    private var accepting = true
    private var failureMessage: String?

    // 空から非空への遷移と最初の失敗だけを通知し、main queue への通知も有界にする。
    func enqueue(_ buffer: AVAudioPCMBuffer) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard accepting, buffer.frameLength > 0 else { return false }
        guard buffers.count < Self.maximumBufferCount else {
            failureMessage = "音声認識待ちの音声が上限を超えました"
            accepting = false
            return true
        }
        do {
            let copied = try deepCopyAudioBuffer(buffer)
            let needsNotification = buffers.isEmpty
            buffers.append(copied)
            return needsNotification
        } catch {
            failureMessage = "音声バッファを保持できませんでした"
            accepting = false
            return true
        }
    }

    var failure: String? {
        lock.lock()
        defer { lock.unlock() }
        return failureMessage
    }

    func takeAll() -> [AVAudioPCMBuffer] {
        lock.lock()
        defer { lock.unlock() }
        let pending = buffers
        buffers.removeAll(keepingCapacity: true)
        return pending
    }

    func stopAccepting() {
        lock.lock()
        defer { lock.unlock() }
        accepting = false
    }

    func discard() {
        lock.lock()
        defer { lock.unlock() }
        accepting = false
        buffers.removeAll()
    }
}
