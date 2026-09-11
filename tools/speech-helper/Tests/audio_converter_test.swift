import AVFoundation
import Foundation

private func conversionInput(_ format: AVAudioFormat, start: Int, frames: Int) -> AVAudioPCMBuffer {
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frames))!
    buffer.frameLength = AVAudioFrameCount(frames)
    for channel in 0..<Int(format.channelCount) {
        for index in 0..<frames {
            buffer.floatChannelData![channel][index] = Float(sin(Double(start + index) * 2 * .pi * 440 / format.sampleRate)) * 0.25
        }
    }
    return buffer
}

private func convertedSamples(input: AVAudioFormat, frames: Int, chunk: Int) throws -> (samples: [Int16], beforeFinish: Int) {
    let output = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: false)!
    let converter = try SpeechAudioConverter(inputFormat: input, outputFormat: output)
    var samples: [Int16] = []
    let receive: (AVAudioPCMBuffer) -> Void = { buffer in
        expect(buffer.format == output && buffer.frameLength > 0, "認識器が受け取れる出力形式に揃える")
        samples.append(contentsOf: UnsafeBufferPointer(start: buffer.int16ChannelData![0], count: Int(buffer.frameLength)))
    }
    for start in stride(from: 0, to: frames, by: chunk) {
        try converter.append(conversionInput(input, start: start, frames: min(chunk, frames - start)), receive: receive)
    }
    let beforeFinish = samples.count
    try converter.finish(receive: receive)
    let endedCount = samples.count
    try converter.finish(receive: receive)
    expect(samples.count == endedCount, "converter の EOS は一度だけ")
    return (samples, beforeFinish)
}

func testAudioConverter() {
    runTest("48k/44.1k の mono/stereo を継続変換し、分割境界で音声を欠落させない") {
        for rate in [48_000.0, 44_100.0] {
            for channels: AVAudioChannelCount in [1, 2] {
                let input = AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels)!
                let frames = Int(rate * 1.01)
                let whole = try convertedSamples(input: input, frames: frames, chunk: frames)
                let streamed = try convertedSamples(input: input, frames: frames, chunk: 1_024)
                expect(streamed.samples == whole.samples, "buffer 分割で resampler の履歴を失わない: \(rate)Hz / \(channels)ch")
                expect(streamed.samples.count == 16_160, "全入力に対応する出力フレーム数を保持する")
                expect(streamed.samples.contains(where: { $0 != 0 }), "音声を無音に置換しない")
                expect(streamed.beforeFinish < streamed.samples.count, "EOS でリサンプラーの末尾を drain する")
            }
        }
    }

    runTest("1フレームずつの入力でも末尾まで同じ PCM に変換する") {
        let input = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let whole = try convertedSamples(input: input, frames: 1_027, chunk: 1_027)
        let streamed = try convertedSamples(input: input, frames: 1_027, chunk: 1)
        expect(streamed.samples == whole.samples && !streamed.samples.isEmpty, "入力が変換器の窓幅より短くても保持する")
    }

    runTest("入力なしの EOS とその後の append を区別する") {
        let input = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let output = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: false)!
        let converter = try SpeechAudioConverter(inputFormat: input, outputFormat: output)
        var count = 0
        try converter.finish { count += Int($0.frameLength) }
        expect(count == 0, "無音サンプルを捏造しない")
        do {
            try converter.append(conversionInput(input, start: 0, frames: 1)) { _ in }
            expect(false, "EOS 後の音声を受け付けない")
        } catch SpeechAudioConversionError.inputClosed {}
    }

    runTest("途中のフォーマット変更・空バッファを黙って受け付けない") {
        let input = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let wrong = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!
        let output = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: false)!
        for buffer in [conversionInput(wrong, start: 0, frames: 1), AVAudioPCMBuffer(pcmFormat: input, frameCapacity: 1)!] {
            let converter = try SpeechAudioConverter(inputFormat: input, outputFormat: output)
            do {
                try converter.append(buffer) { _ in }
                expect(false, "format と frameLength を検証する")
            } catch SpeechAudioConversionError.invalidFormat {}
        }
    }

    runTest("出力側の上限・転送失敗を呼出元に伝える") {
        enum Backpressure: Error { case full }
        let input = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let output = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 1, interleaved: false)!
        let converter = try SpeechAudioConverter(inputFormat: input, outputFormat: output)
        do {
            try converter.append(conversionInput(input, start: 0, frames: 4_096)) { _ in throw Backpressure.full }
            expect(false, "受信できなかった音声を成功扱いにしない")
        } catch Backpressure.full {}
    }
}
