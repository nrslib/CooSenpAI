import AVFoundation
import Foundation

private func littleEndianBytes<T: FixedWidthInteger>(_ value: T) -> [UInt8] {
    var littleEndianValue = value.littleEndian
    return withUnsafeBytes(of: &littleEndianValue) { Array($0) }
}

private func makePCM16Wav(samples: [Int16], sampleRate: UInt32) -> Data {
    let dataByteCount = UInt32(samples.count * MemoryLayout<Int16>.size)
    let riffByteCount = UInt32(36) + dataByteCount
    var data = Data("RIFF".utf8)
    data.append(contentsOf: littleEndianBytes(riffByteCount))
    data.append(contentsOf: Data("WAVE".utf8))
    data.append(contentsOf: Data("fmt ".utf8))
    data.append(contentsOf: littleEndianBytes(UInt32(16)))
    data.append(contentsOf: littleEndianBytes(UInt16(1)))
    data.append(contentsOf: littleEndianBytes(UInt16(1)))
    data.append(contentsOf: littleEndianBytes(sampleRate))
    data.append(contentsOf: littleEndianBytes(sampleRate * 2))
    data.append(contentsOf: littleEndianBytes(UInt16(2)))
    data.append(contentsOf: littleEndianBytes(UInt16(16)))
    data.append(contentsOf: Data("data".utf8))
    data.append(contentsOf: littleEndianBytes(dataByteCount))
    for sample in samples {
        data.append(contentsOf: littleEndianBytes(sample))
    }
    return data
}

func testDebugInputWav() throws {
    let path = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-debug-input-\(UUID().uuidString).wav")
    defer { try? FileManager.default.removeItem(at: path) }
    try makePCM16Wav(samples: [0, 1_000, -1_000, 0], sampleRate: 16_000)
        .write(to: path)

    let player = try DebugInputWavPlayer(path: path.path, playbackRate: 1.0)
    assert(player.source == .microphone)
    assert(player.format.sampleRate == 16_000)
    assert(player.format.channelCount == 1)
    assert(player.frameLength == 4)

    let firstBufferReceived = DispatchSemaphore(value: 0)
    let playbackCompleted = DispatchSemaphore(value: 0)
    var receivedFrameLength: AVAudioFrameCount = 0
    var completedSuccessfully = false
    player.start(
        onBuffer: { buffer in
            receivedFrameLength = buffer.frameLength
            firstBufferReceived.signal()
        },
        onCompletion: { result in
            switch result {
            case .success:
                completedSuccessfully = true
            case let .failure(error):
                assertionFailure("WAV の読み込みに失敗しました: \(error)")
            }
            playbackCompleted.signal()
        }
    )
    assert(firstBufferReceived.wait(timeout: .now() + .seconds(1)) == .success)
    assert(receivedFrameLength == 4)
    assert(playbackCompleted.wait(timeout: .now() + .seconds(1)) == .success)
    assert(completedSuccessfully)
}
