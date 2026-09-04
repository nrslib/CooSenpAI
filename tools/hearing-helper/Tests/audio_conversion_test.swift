import AVFoundation
import AudioToolbox
import CoreMedia

private func makeFloatSampleBuffer(
    _ samples: [Float],
    sampleCount: Int
) throws -> CMSampleBuffer {
    var streamDescription = AudioStreamBasicDescription(
        mSampleRate: 48_000,
        mFormatID: kAudioFormatLinearPCM,
        mFormatFlags: kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
        mBytesPerPacket: UInt32(MemoryLayout<Float>.size),
        mFramesPerPacket: 1,
        mBytesPerFrame: UInt32(MemoryLayout<Float>.size),
        mChannelsPerFrame: 1,
        mBitsPerChannel: UInt32(MemoryLayout<Float>.size * 8),
        mReserved: 0
    )
    var formatDescription: CMAudioFormatDescription?
    let formatStatus = CMAudioFormatDescriptionCreate(
        allocator: kCFAllocatorDefault,
        asbd: &streamDescription,
        layoutSize: 0,
        layout: nil,
        magicCookieSize: 0,
        magicCookie: nil,
        extensions: nil,
        formatDescriptionOut: &formatDescription
    )
    guard formatStatus == noErr, let formatDescription else {
        throw TestError("フォーマット記述の作成に失敗しました: \(formatStatus)")
    }

    let byteCount = samples.count * MemoryLayout<Float>.size
    var dataBuffer: CMBlockBuffer?
    let blockStatus = CMBlockBufferCreateWithMemoryBlock(
        allocator: kCFAllocatorDefault,
        memoryBlock: nil,
        blockLength: byteCount,
        blockAllocator: kCFAllocatorDefault,
        customBlockSource: nil,
        offsetToData: 0,
        dataLength: byteCount,
        flags: kCMBlockBufferAssureMemoryNowFlag,
        blockBufferOut: &dataBuffer
    )
    guard blockStatus == noErr, let dataBuffer else {
        throw TestError("データ buffer の作成に失敗しました: \(blockStatus)")
    }
    let replaceStatus = samples.withUnsafeBytes { rawSamples in
        CMBlockBufferReplaceDataBytes(
            with: rawSamples.baseAddress!,
            blockBuffer: dataBuffer,
            offsetIntoDestination: 0,
            dataLength: byteCount
        )
    }
    guard replaceStatus == noErr else {
        throw TestError("データ buffer への書き込みに失敗しました: \(replaceStatus)")
    }
    var copiedBytes = [UInt8](repeating: 0, count: byteCount)
    let copiedStatus = copiedBytes.withUnsafeMutableBytes { rawBytes in
        CMBlockBufferCopyDataBytes(
            dataBuffer,
            atOffset: 0,
            dataLength: byteCount,
            destination: rawBytes.baseAddress!
        )
    }
    guard copiedStatus == noErr else {
        throw TestError("データ buffer からの読み出しに失敗しました: \(copiedStatus)")
    }
    let copiedSamples = copiedBytes.withUnsafeBytes { rawBytes in
        Array(rawBytes.bindMemory(to: Float.self))
    }
    guard copiedSamples == samples else {
        throw TestError("データ buffer の内容が不正です: \(copiedSamples)")
    }

    var timing = CMSampleTimingInfo(
        duration: CMTime(value: 1, timescale: 48_000),
        presentationTimeStamp: .zero,
        decodeTimeStamp: .invalid
    )
    var sampleSize = MemoryLayout<Float>.size
    var sampleBuffer: CMSampleBuffer?
    let sampleStatus = CMSampleBufferCreateReady(
        allocator: kCFAllocatorDefault,
        dataBuffer: dataBuffer,
        formatDescription: formatDescription,
        sampleCount: sampleCount,
        sampleTimingEntryCount: 1,
        sampleTimingArray: &timing,
        sampleSizeEntryCount: 1,
        sampleSizeArray: &sampleSize,
        sampleBufferOut: &sampleBuffer
    )
    guard sampleStatus == noErr, let sampleBuffer else {
        throw TestError("CMSampleBuffer の作成に失敗しました: \(sampleStatus)")
    }
    return sampleBuffer
}

private struct TestError: Error {
    let message: String

    init(_ message: String) {
        self.message = message
    }
}

private func makeStereoFloatBuffer(interleaved: Bool) -> AVAudioPCMBuffer {
    let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 2,
        interleaved: interleaved
    )!
    let leftSamples: [Float] = [0.25, -0.5, 0.75, -1.0]
    let rightSamples: [Float] = [0.25, -0.5, 0.25, -0.5]
    let buffer = AVAudioPCMBuffer(
        pcmFormat: format,
        frameCapacity: AVAudioFrameCount(leftSamples.count)
    )!
    buffer.frameLength = AVAudioFrameCount(leftSamples.count)
    let audioBuffers = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
    if interleaved {
        let samples = audioBuffers[0].mData!.assumingMemoryBound(to: Float.self)
        for frame in leftSamples.indices {
            samples[frame * 2] = leftSamples[frame]
            samples[frame * 2 + 1] = rightSamples[frame]
        }
    } else {
        let left = audioBuffers[0].mData!.assumingMemoryBound(to: Float.self)
        let right = audioBuffers[1].mData!.assumingMemoryBound(to: Float.self)
        leftSamples.withUnsafeBufferPointer { source in
            left.update(from: source.baseAddress!, count: leftSamples.count)
        }
        rightSamples.withUnsafeBufferPointer { source in
            right.update(from: source.baseAddress!, count: rightSamples.count)
        }
    }
    return buffer
}

private func testMonoFloat32AudioBuffer(interleaved: Bool) throws {
    let converted = try monoFloat32AudioBuffer(
        from: makeStereoFloatBuffer(interleaved: interleaved)
    )
    guard converted.format.channelCount == 1,
          converted.format.commonFormat == .pcmFormatFloat32,
          !converted.format.isInterleaved,
          let samples = converted.floatChannelData?[0] else {
        throw TestError("ステレオ音声を mono float32 に変換できません")
    }
    let expected: [Float] = [0.25, -0.5, 0.5, -0.75]
    assert(converted.frameLength == AVAudioFrameCount(expected.count))
    for index in expected.indices {
        assert(abs(samples[index] - expected[index]) < 0.000_001)
    }
}

func testMonoFloat32AudioBufferConversion() throws {
    try testMonoFloat32AudioBuffer(interleaved: true)
    try testMonoFloat32AudioBuffer(interleaved: false)
}

func testAudioConversion() throws {
    let sampleBuffer = try makeFloatSampleBuffer(
        [0.25, -0.5, 0.75, -1.0],
        sampleCount: 4
    )
    let converted = try pcmBuffer(from: sampleBuffer)
    guard let channelData = converted.floatChannelData else {
        throw TestError("Float32 の channel data がありません")
    }
    assert(converted.frameLength == 4)
    assert(abs(channelData[0][0] - 0.25) < 0.000_001)
    assert(abs(channelData[0][1] + 0.5) < 0.000_001)
    assert(abs(channelData[0][2] - 0.75) < 0.000_001)
    assert(abs(channelData[0][3] + 1.0) < 0.000_001)

    let partialSampleBuffer = try makeFloatSampleBuffer(
        [0.25, -0.5],
        sampleCount: 4
    )
    do {
        _ = try pcmBuffer(from: partialSampleBuffer)
        assertionFailure("部分的な CMSampleBuffer は変換してはいけません")
    } catch let error as PCMBufferConversionError {
        switch error {
        case let .bufferByteSizeMismatch(_, expected, actual):
            assert(expected == 16)
            assert(actual == 8)
        case let .backingDataTooShort(expected, actual):
            assert(expected == 16)
            assert(actual == 8)
        default:
            throw TestError("部分 buffer のエラー分類が不正です: \(error)")
        }
    }
    try testMonoFloat32AudioBufferConversion()
}
