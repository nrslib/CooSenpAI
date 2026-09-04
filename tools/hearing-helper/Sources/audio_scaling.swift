import AVFoundation

struct AudioSampleClampResult: Equatable {
    let value: Double
    let wasOutOfRange: Bool
    let wasNonFinite: Bool

    init(value: Double, wasOutOfRange: Bool, wasNonFinite: Bool = false) {
        self.value = value
        self.wasOutOfRange = wasOutOfRange
        self.wasNonFinite = wasNonFinite
    }
}

struct AudioSampleClampSummary: Equatable {
    let sampleCount: UInt64
    let outOfRangeCount: UInt64
    let nonFiniteCount: UInt64

    var hasAnomaly: Bool {
        outOfRangeCount > 0 || nonFiniteCount > 0
    }

    var clippedSampleCount: UInt64 {
        outOfRangeCount + nonFiniteCount
    }

    var clippedSampleRatio: Double {
        guard sampleCount > 0 else { return 0 }
        return Double(clippedSampleCount) / Double(sampleCount)
    }
}

func clampedAudioSample(_ value: Double) -> AudioSampleClampResult {
    guard value.isFinite else {
        return AudioSampleClampResult(
            value: 0,
            wasOutOfRange: false,
            wasNonFinite: true
        )
    }

    let clampedValue = min(max(value, -1), 1)
    return AudioSampleClampResult(
        value: clampedValue,
        wasOutOfRange: clampedValue != value
    )
}

func clampAudioSamples(in buffer: AVAudioPCMBuffer) -> AudioSampleClampSummary {
    let commonFormat = buffer.format.commonFormat
    let bytesPerSample: Int
    switch commonFormat {
    case .pcmFormatFloat32: bytesPerSample = MemoryLayout<Float>.size
    case .pcmFormatFloat64: bytesPerSample = MemoryLayout<Double>.size
    case .pcmFormatInt16, .pcmFormatInt32, .otherFormat:
        return AudioSampleClampSummary(
            sampleCount: 0,
            outOfRangeCount: 0,
            nonFiniteCount: 0
        )
    @unknown default:
        return AudioSampleClampSummary(
            sampleCount: 0,
            outOfRangeCount: 0,
            nonFiniteCount: 0
        )
    }

    var sampleCount: UInt64 = 0
    var outOfRangeCount: UInt64 = 0
    var nonFiniteCount: UInt64 = 0
    let audioBuffers = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
    for audioBuffer in audioBuffers {
        guard let data = audioBuffer.mData else { continue }
        let channelCount = max(Int(audioBuffer.mNumberChannels), 1)
        let frameCount = Int(buffer.frameLength)
        let count = min(
            Int(audioBuffer.mDataByteSize) / bytesPerSample,
            frameCount * channelCount
        )
        guard count > 0 else { continue }

        switch commonFormat {
        case .pcmFormatFloat32:
            let samples = data.assumingMemoryBound(to: Float.self)
            for index in 0..<count {
                let result = clampedAudioSample(Double(samples[index]))
                samples[index] = Float(result.value)
                sampleCount &+= 1
                if result.wasOutOfRange { outOfRangeCount &+= 1 }
                if result.wasNonFinite { nonFiniteCount &+= 1 }
            }
        case .pcmFormatFloat64:
            let samples = data.assumingMemoryBound(to: Double.self)
            for index in 0..<count {
                let result = clampedAudioSample(samples[index])
                samples[index] = result.value
                sampleCount &+= 1
                if result.wasOutOfRange { outOfRangeCount &+= 1 }
                if result.wasNonFinite { nonFiniteCount &+= 1 }
            }
        case .pcmFormatInt16, .pcmFormatInt32, .otherFormat:
            break
        @unknown default:
            break
        }
    }
    return AudioSampleClampSummary(
        sampleCount: sampleCount,
        outOfRangeCount: outOfRangeCount,
        nonFiniteCount: nonFiniteCount
    )
}
