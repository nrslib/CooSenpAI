import AVFoundation
import Foundation

enum SpeechAudioConversionError: Error {
    case invalidFormat
    case allocationFailed
    case conversionFailed
    case inputClosed
}

final class SpeechAudioConverter {
    private let inputFormat: AVAudioFormat
    let outputFormat: AVAudioFormat
    private let converter: AVAudioConverter
    private var ended = false

    init(inputFormat: AVAudioFormat, outputFormat: AVAudioFormat) throws {
        guard inputFormat.sampleRate.isFinite, inputFormat.sampleRate > 0,
              outputFormat.sampleRate.isFinite, outputFormat.sampleRate > 0,
              let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw SpeechAudioConversionError.invalidFormat
        }
        self.inputFormat = inputFormat
        self.outputFormat = outputFormat
        self.converter = converter
    }

    func append(_ input: AVAudioPCMBuffer, receive: (AVAudioPCMBuffer) throws -> Void) throws {
        guard !ended else { throw SpeechAudioConversionError.inputClosed }
        guard input.format == inputFormat, input.frameLength > 0 else {
            throw SpeechAudioConversionError.invalidFormat
        }
        try convert(input, receive: receive)
    }

    func finish(receive: (AVAudioPCMBuffer) throws -> Void) throws {
        guard !ended else { return }
        ended = true
        try convert(nil, receive: receive)
    }

    private func convert(_ input: AVAudioPCMBuffer?, receive: (AVAudioPCMBuffer) throws -> Void) throws {
        var supplied = false
        while true {
            guard let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: 4_096) else {
                throw SpeechAudioConversionError.allocationFailed
            }
            var error: NSError?
            let status = converter.convert(to: output, error: &error) { _, status in
                if let input, !supplied {
                    supplied = true
                    status.pointee = .haveData
                    return input
                }
                // buffer ごとに EOS を送るとリサンプラーの履歴と末尾が途切れる。
                status.pointee = input == nil ? .endOfStream : .noDataNow
                return nil
            }
            if let error { throw error }
            if status == .error { throw SpeechAudioConversionError.conversionFailed }
            if output.frameLength > 0 { try receive(output) }
            switch status {
            case .haveData:
                guard output.frameLength > 0 else { throw SpeechAudioConversionError.conversionFailed }
            case .inputRanDry, .endOfStream: return
            case .error: throw SpeechAudioConversionError.conversionFailed
            @unknown default: throw SpeechAudioConversionError.conversionFailed
            }
        }
    }
}
