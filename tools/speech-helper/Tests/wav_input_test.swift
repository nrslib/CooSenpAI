import AVFoundation
import Foundation

func testWavInput() {
    runTest("共有 WAV 入力はすべてのフレームを一度だけ渡し終端を通知する") {
        let path = FileManager.default.temporaryDirectory.appendingPathComponent("speech-wav-\(UUID().uuidString).wav")
        defer { try? FileManager.default.removeItem(at: path) }
        do {
            let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4_801)!
            buffer.frameLength = 4_801
            let expected = (0..<4_801).map { Float($0 % 1_024) / 1_024 }
            for (index, sample) in expected.enumerated() { buffer.floatChannelData![0][index] = sample }
            do {
                let file = try AVAudioFile(forWriting: path, settings: format.settings)
                try file.write(from: buffer)
            }
            let player = try DebugInputWavPlayer(path: path.path, playbackRate: 1)
            let completed = DispatchSemaphore(value: 0)
            var samples: [Float] = []
            var completionCount = 0
            var succeeded = false
            player.start(
                onBuffer: { buffer in
                    samples += Array(UnsafeBufferPointer(start: buffer.floatChannelData![0], count: Int(buffer.frameLength)))
                },
                onCompletion: { result in
                    completionCount += 1
                    if case .success = result { succeeded = true }
                    completed.signal()
                }
            )
            expect(completed.wait(timeout: .now() + 2) == .success, "WAV の終端通知を受け取る")
            expect(samples == expected && player.frameLength == 4_801, "最後の端数も欠落・重複させない")
            expect(succeeded && completionCount == 1, "正常完了を一度だけ通知する")
            player.stop()
        } catch {
            expect(false, "WAV 入力テストを開始できませんでした: \(error)")
        }
    }
}
