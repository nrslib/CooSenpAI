import AVFoundation

struct SpeechAudioGainResult: Equatable {
    let inputPeak: Double
    let gain: Double
}

enum SpeechAudioGainError: Error {
    case unsupportedFormat
    case nonFiniteSample
}

enum SpeechAudioGain {
    static let targetPeak = 0.1
    static let maximumGain = 32.0

    static func apply(to buffer: AVAudioPCMBuffer) throws -> SpeechAudioGainResult {
        let inputPeak = try peak(of: buffer)
        guard inputPeak > 0 else {
            return SpeechAudioGainResult(inputPeak: 0, gain: 1)
        }

        let gain = min(max(targetPeak / inputPeak, 1), maximumGain)
        if gain > 1 {
            try scale(buffer, by: gain)
        }
        return SpeechAudioGainResult(inputPeak: inputPeak, gain: gain)
    }

    private static func peak(of buffer: AVAudioPCMBuffer) throws -> Double {
        let bytesPerSample: Int
        switch buffer.format.commonFormat {
        case .pcmFormatFloat32: bytesPerSample = MemoryLayout<Float>.size
        case .pcmFormatFloat64: bytesPerSample = MemoryLayout<Double>.size
        case .pcmFormatInt16: bytesPerSample = MemoryLayout<Int16>.size
        case .pcmFormatInt32: bytesPerSample = MemoryLayout<Int32>.size
        case .otherFormat: throw SpeechAudioGainError.unsupportedFormat
        @unknown default: throw SpeechAudioGainError.unsupportedFormat
        }

        var result = 0.0
        for audioBuffer in UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList) {
            guard let data = audioBuffer.mData else { continue }
            let channelCount = max(Int(audioBuffer.mNumberChannels), 1)
            let count = min(
                Int(audioBuffer.mDataByteSize) / bytesPerSample,
                Int(buffer.frameLength) * channelCount
            )
            guard count > 0 else { continue }
            switch buffer.format.commonFormat {
            case .pcmFormatFloat32:
                let samples = data.assumingMemoryBound(to: Float.self)
                for index in 0..<count {
                    let value = Double(samples[index])
                    guard value.isFinite else { throw SpeechAudioGainError.nonFiniteSample }
                    result = max(result, abs(value))
                }
            case .pcmFormatFloat64:
                let samples = data.assumingMemoryBound(to: Double.self)
                for index in 0..<count {
                    let value = samples[index]
                    guard value.isFinite else { throw SpeechAudioGainError.nonFiniteSample }
                    result = max(result, abs(value))
                }
            case .pcmFormatInt16:
                let samples = data.assumingMemoryBound(to: Int16.self)
                for index in 0..<count {
                    result = max(result, abs(Double(samples[index])) / 32_768.0)
                }
            case .pcmFormatInt32:
                let samples = data.assumingMemoryBound(to: Int32.self)
                for index in 0..<count {
                    result = max(result, abs(Double(samples[index])) / 2_147_483_648.0)
                }
            case .otherFormat:
                throw SpeechAudioGainError.unsupportedFormat
            @unknown default:
                throw SpeechAudioGainError.unsupportedFormat
            }
        }
        return result
    }

    private static func scale(_ buffer: AVAudioPCMBuffer, by gain: Double) throws {
        for audioBuffer in UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList) {
            guard let data = audioBuffer.mData else { continue }
            let channelCount = max(Int(audioBuffer.mNumberChannels), 1)
            let bytesPerSample: Int
            switch buffer.format.commonFormat {
            case .pcmFormatFloat32: bytesPerSample = MemoryLayout<Float>.size
            case .pcmFormatFloat64: bytesPerSample = MemoryLayout<Double>.size
            case .pcmFormatInt16: bytesPerSample = MemoryLayout<Int16>.size
            case .pcmFormatInt32: bytesPerSample = MemoryLayout<Int32>.size
            case .otherFormat: throw SpeechAudioGainError.unsupportedFormat
            @unknown default: throw SpeechAudioGainError.unsupportedFormat
            }
            let count = min(
                Int(audioBuffer.mDataByteSize) / bytesPerSample,
                Int(buffer.frameLength) * channelCount
            )
            guard count > 0 else { continue }

            switch buffer.format.commonFormat {
            case .pcmFormatFloat32:
                let samples = data.assumingMemoryBound(to: Float.self)
                for index in 0..<count {
                    let value = Double(samples[index])
                    guard value.isFinite else { throw SpeechAudioGainError.nonFiniteSample }
                    samples[index] = Float(min(max(value * gain, -1), 1))
                }
            case .pcmFormatFloat64:
                let samples = data.assumingMemoryBound(to: Double.self)
                for index in 0..<count {
                    let value = samples[index]
                    guard value.isFinite else { throw SpeechAudioGainError.nonFiniteSample }
                    samples[index] = min(max(value * gain, -1), 1)
                }
            case .pcmFormatInt16:
                let samples = data.assumingMemoryBound(to: Int16.self)
                for index in 0..<count {
                    let value = Double(samples[index]) * gain
                    samples[index] = Int16(min(max(value.rounded(), -32_768), 32_767))
                }
            case .pcmFormatInt32:
                let samples = data.assumingMemoryBound(to: Int32.self)
                for index in 0..<count {
                    let value = Double(samples[index]) * gain
                    samples[index] = Int32(min(max(value.rounded(), -2_147_483_648), 2_147_483_647))
                }
            case .otherFormat:
                throw SpeechAudioGainError.unsupportedFormat
            @unknown default:
                throw SpeechAudioGainError.unsupportedFormat
            }
        }
    }
}
