import AVFoundation

private func makeFloatBuffer(_ samples: [Float]) -> AVAudioPCMBuffer {
    let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!
    let buffer = AVAudioPCMBuffer(
        pcmFormat: format,
        frameCapacity: AVAudioFrameCount(samples.count)
    )!
    buffer.frameLength = AVAudioFrameCount(samples.count)
    samples.withUnsafeBufferPointer { source in
        buffer.floatChannelData![0].update(
            from: source.baseAddress!,
            count: samples.count
        )
    }
    return buffer
}

func testAudioBufferCopy() throws {
    let source = makeFloatBuffer([0.25, -0.5, 0.75, -1])
    let copied = try deepCopyAudioBuffer(source)
    let sourceSamples = source.floatChannelData![0]
    sourceSamples[0] = 0
    sourceSamples[1] = 0
    sourceSamples[2] = 0
    sourceSamples[3] = 0

    assert(copied.frameLength == 4)
    let copiedSamples = copied.floatChannelData![0]
    assert(abs(copiedSamples[0] - 0.25) < 0.000_001)
    assert(abs(copiedSamples[1] + 0.5) < 0.000_001)
    assert(abs(copiedSamples[2] - 0.75) < 0.000_001)
    assert(abs(copiedSamples[3] + 1) < 0.000_001)

    var pending = PendingAudioWindow<AVAudioPCMBuffer>(capacityNanoseconds: 1_000)
    assert(pending.append(copied, durationNanoseconds: 1_000))
    sourceSamples[0] = 1
    let pendingBuffers = pending.removeAll()
    assert(pendingBuffers.count == 1)
    let pendingSamples = pendingBuffers[0].floatChannelData![0]
    assert(abs(pendingSamples[0] - 0.25) < 0.000_001)
}
