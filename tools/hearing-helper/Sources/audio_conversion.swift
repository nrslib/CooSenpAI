import AVFoundation
import AudioToolbox
import CoreMedia

enum PCMBufferConversionError: LocalizedError {
    case invalidSampleBuffer
    case dataNotReady
    case missingFormatDescription
    case missingStreamDescription
    case invalidAudioFormat
    case emptySampleBuffer
    case invalidAudioLayout
    case audioBufferListSize(OSStatus)
    case audioBufferList(OSStatus)
    case bufferCountMismatch(expected: Int, actual: Int)
    case bufferChannelCountMismatch(index: Int, expected: Int, actual: Int)
    case bufferByteSizeMismatch(index: Int, expected: Int, actual: Int)
    case backingDataTooShort(expected: Int, actual: Int)
    case missingDestinationData(index: Int)
    case missingDataBuffer
    case dataCopy(OSStatus, index: Int)

    var errorDescription: String? {
        switch self {
        case .invalidSampleBuffer:
            return "CMSampleBuffer が無効です"
        case .dataNotReady:
            return "CMSampleBuffer の音声データが未準備です"
        case .missingFormatDescription:
            return "CMSampleBuffer に音声フォーマット記述がありません"
        case .missingStreamDescription:
            return "CMSampleBuffer に音声ストリーム記述がありません"
        case .invalidAudioFormat:
            return "CMSampleBuffer の音声フォーマットを PCM に変換できません"
        case .emptySampleBuffer:
            return "CMSampleBuffer に音声フレームがありません"
        case .invalidAudioLayout:
            return "CMSampleBuffer の音声レイアウトが不正です"
        case let .audioBufferListSize(status):
            return "音声 buffer list のサイズ取得に失敗しました: status=\(status)"
        case let .audioBufferList(status):
            return "音声 buffer list の取得に失敗しました: status=\(status)"
        case let .bufferCountMismatch(expected, actual):
            return "音声 buffer 数がフォーマットと一致しません: expected=\(expected) actual=\(actual)"
        case let .bufferChannelCountMismatch(index, expected, actual):
            return "音声 buffer のチャンネル数がフォーマットと一致しません: index=\(index) expected=\(expected) actual=\(actual)"
        case let .bufferByteSizeMismatch(index, expected, actual):
            return "音声 buffer の実データサイズがフレーム数と一致しません: index=\(index) expectedBytes=\(expected) actualBytes=\(actual)"
        case let .backingDataTooShort(expected, actual):
            return "CMBlockBuffer の実データが音声フレーム全体を含みません: expectedBytes=\(expected) actualBytes=\(actual)"
        case let .missingDestinationData(index):
            return "音声 buffer のコピー先データポインタがありません: index=\(index)"
        case .missingDataBuffer:
            return "CMSampleBuffer に CMBlockBuffer がありません"
        case let .dataCopy(status, index):
            return "CMBlockBuffer から音声データをコピーできません: status=\(status) index=\(index)"
        }
    }
}

private let audioBufferListFlags = UInt32(kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment)

func pcmBuffer(from sampleBuffer: CMSampleBuffer) throws -> AVAudioPCMBuffer {
    guard sampleBuffer.isValid else {
        throw PCMBufferConversionError.invalidSampleBuffer
    }
    guard CMSampleBufferDataIsReady(sampleBuffer) else {
        throw PCMBufferConversionError.dataNotReady
    }
    guard let formatDescription = CMSampleBufferGetFormatDescription(sampleBuffer) else {
        throw PCMBufferConversionError.missingFormatDescription
    }
    guard let streamDescription = CMAudioFormatDescriptionGetStreamBasicDescription(formatDescription) else {
        throw PCMBufferConversionError.missingStreamDescription
    }
    guard let format = AVAudioFormat(streamDescription: streamDescription) else {
        throw PCMBufferConversionError.invalidAudioFormat
    }
    let frameCount = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
    guard frameCount > 0 else {
        throw PCMBufferConversionError.emptySampleBuffer
    }
    let channelCount = Int(format.channelCount)
    let bytesPerFrame = Int(streamDescription.pointee.mBytesPerFrame)
    guard channelCount > 0, bytesPerFrame > 0 else {
        throw PCMBufferConversionError.invalidAudioLayout
    }
    let expectedBufferCount = format.isInterleaved ? 1 : channelCount
    let expectedBytesPerBuffer: Int
    let (byteCount, byteCountOverflow) = Int(frameCount).multipliedReportingOverflow(by: bytesPerFrame)
    guard !byteCountOverflow else {
        throw PCMBufferConversionError.invalidAudioLayout
    }
    expectedBytesPerBuffer = byteCount
    guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount) else {
        throw PCMBufferConversionError.invalidAudioFormat
    }
    // mutableAudioBufferList の mDataByteSize は frameCapacity を示す実装もあるが、
    // 初期 frameLength が 0 のバッファでは 0 のままになるため、コピー前に長さを確定する。
    buffer.frameLength = frameCount

    var requiredSize = 0
    var retainedBlockBuffer: CMBlockBuffer?
    let sizeStatus = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer,
        bufferListSizeNeededOut: &requiredSize,
        bufferListOut: nil,
        bufferListSize: 0,
        blockBufferAllocator: nil,
        blockBufferMemoryAllocator: nil,
        flags: audioBufferListFlags,
        blockBufferOut: &retainedBlockBuffer
    )
    guard sizeStatus == noErr, requiredSize > 0 else {
        throw PCMBufferConversionError.audioBufferListSize(sizeStatus)
    }

    let rawList = UnsafeMutableRawPointer.allocate(
        byteCount: requiredSize,
        alignment: MemoryLayout<AudioBufferList>.alignment
    )
    defer { rawList.deallocate() }
    let sourceList = rawList.bindMemory(to: AudioBufferList.self, capacity: 1)
    let listStatus = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer,
        bufferListSizeNeededOut: nil,
        bufferListOut: sourceList,
        bufferListSize: requiredSize,
        blockBufferAllocator: nil,
        blockBufferMemoryAllocator: nil,
        flags: audioBufferListFlags,
        blockBufferOut: &retainedBlockBuffer
    )
    guard listStatus == noErr else {
        throw PCMBufferConversionError.audioBufferList(listStatus)
    }

    let sourceBuffers = UnsafeMutableAudioBufferListPointer(sourceList)
    let destinationBuffers = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
    guard destinationBuffers.count == expectedBufferCount else {
        throw PCMBufferConversionError.bufferCountMismatch(
            expected: expectedBufferCount,
            actual: destinationBuffers.count
        )
    }
    guard sourceBuffers.count == expectedBufferCount else {
        throw PCMBufferConversionError.bufferCountMismatch(
            expected: expectedBufferCount,
            actual: sourceBuffers.count
        )
    }
    let expectedChannelCount = format.isInterleaved ? channelCount : 1
    let (expectedTotalBytes, totalByteCountOverflow) = expectedBytesPerBuffer
        .multipliedReportingOverflow(by: expectedBufferCount)
    guard !totalByteCountOverflow else {
        throw PCMBufferConversionError.invalidAudioLayout
    }
    if let retainedBlockBuffer {
        let actualByteCount = CMBlockBufferGetDataLength(retainedBlockBuffer)
        guard actualByteCount >= expectedTotalBytes else {
            throw PCMBufferConversionError.backingDataTooShort(
                expected: expectedTotalBytes,
                actual: actualByteCount
            )
        }
    }

    for index in sourceBuffers.indices {
        let source = sourceBuffers[index]
        let destination = destinationBuffers[index]
        guard Int(source.mNumberChannels) == expectedChannelCount else {
            throw PCMBufferConversionError.bufferChannelCountMismatch(
                index: index,
                expected: expectedChannelCount,
                actual: Int(source.mNumberChannels)
            )
        }
        guard Int(destination.mNumberChannels) == expectedChannelCount else {
            throw PCMBufferConversionError.bufferChannelCountMismatch(
                index: index,
                expected: expectedChannelCount,
                actual: Int(destination.mNumberChannels)
            )
        }
        guard let destinationData = destination.mData else {
            throw PCMBufferConversionError.missingDestinationData(index: index)
        }
        let sourceByteCount = Int(source.mDataByteSize)
        guard sourceByteCount == expectedBytesPerBuffer else {
            throw PCMBufferConversionError.bufferByteSizeMismatch(
                index: index,
                expected: expectedBytesPerBuffer,
                actual: sourceByteCount
            )
        }
        let destinationByteCount = Int(destination.mDataByteSize)
        guard destinationByteCount == expectedBytesPerBuffer else {
            throw PCMBufferConversionError.bufferByteSizeMismatch(
                index: index,
                expected: expectedBytesPerBuffer,
                actual: destinationByteCount
            )
        }
        if let sourceData = source.mData {
            memcpy(destinationData, sourceData, expectedBytesPerBuffer)
            continue
        }
        guard let retainedBlockBuffer else {
            throw PCMBufferConversionError.missingDataBuffer
        }
        let (offset, offsetOverflow) = index.multipliedReportingOverflow(by: expectedBytesPerBuffer)
        guard !offsetOverflow else {
            throw PCMBufferConversionError.invalidAudioLayout
        }
        let copyStatus = CMBlockBufferCopyDataBytes(
            retainedBlockBuffer,
            atOffset: offset,
            dataLength: expectedBytesPerBuffer,
            destination: destinationData
        )
        guard copyStatus == noErr else {
            throw PCMBufferConversionError.dataCopy(copyStatus, index: index)
        }
    }
    return buffer
}

enum MonoAudioBufferConversionError: LocalizedError {
    case emptyBuffer
    case invalidAudioFormat
    case unsupportedCommonFormat
    case bufferCountMismatch(expected: Int, actual: Int)
    case bufferChannelCountMismatch(index: Int, expected: Int, actual: Int)
    case missingSourceData(index: Int)
    case sourceDataTooShort(index: Int, expected: Int, actual: Int)
    case allocationFailed
    case missingDestinationData

    var errorDescription: String? {
        switch self {
        case .emptyBuffer:
            return "音声バッファにフレームがありません"
        case .invalidAudioFormat:
            return "音声バッファを mono float32 に変換できるフォーマットではありません"
        case .unsupportedCommonFormat:
            return "音声バッファの PCM フォーマットに対応していません"
        case let .bufferCountMismatch(expected, actual):
            return "音声バッファ数がフォーマットと一致しません: expected=\(expected) actual=\(actual)"
        case let .bufferChannelCountMismatch(index, expected, actual):
            return "音声バッファのチャンネル数がフォーマットと一致しません: index=\(index) expected=\(expected) actual=\(actual)"
        case let .missingSourceData(index):
            return "音声バッファの入力データがありません: index=\(index)"
        case let .sourceDataTooShort(index, expected, actual):
            return "音声バッファの入力データが短すぎます: index=\(index) expectedBytes=\(expected) actualBytes=\(actual)"
        case .allocationFailed:
            return "mono float32 音声バッファを確保できません"
        case .missingDestinationData:
            return "mono float32 音声バッファの出力データがありません"
        }
    }
}

func monoFloat32AudioBuffer(from source: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
    guard source.frameLength > 0 else {
        throw MonoAudioBufferConversionError.emptyBuffer
    }
    guard source.format.sampleRate.isFinite,
          source.format.sampleRate > 0,
          source.format.channelCount > 0 else {
        throw MonoAudioBufferConversionError.invalidAudioFormat
    }
    if source.format.channelCount == 1,
       source.format.commonFormat == .pcmFormatFloat32 {
        return source
    }

    let frameCount = Int(source.frameLength)
    let channelCount = Int(source.format.channelCount)
    let sampleSize: Int
    switch source.format.commonFormat {
    case .pcmFormatFloat32:
        sampleSize = MemoryLayout<Float>.size
    case .pcmFormatFloat64:
        sampleSize = MemoryLayout<Double>.size
    case .pcmFormatInt16:
        sampleSize = MemoryLayout<Int16>.size
    case .pcmFormatInt32:
        sampleSize = MemoryLayout<Int32>.size
    case .otherFormat:
        throw MonoAudioBufferConversionError.unsupportedCommonFormat
    @unknown default:
        throw MonoAudioBufferConversionError.unsupportedCommonFormat
    }

    let expectedBufferCount = source.format.isInterleaved ? 1 : channelCount
    let expectedChannelsPerBuffer = source.format.isInterleaved ? channelCount : 1
    let sampleCount: Int
    if source.format.isInterleaved {
        let result = frameCount.multipliedReportingOverflow(by: channelCount)
        guard !result.overflow else {
            throw MonoAudioBufferConversionError.invalidAudioFormat
        }
        sampleCount = result.partialValue
    } else {
        sampleCount = frameCount
    }
    let byteCount = sampleCount.multipliedReportingOverflow(by: sampleSize)
    guard !byteCount.overflow else {
        throw MonoAudioBufferConversionError.invalidAudioFormat
    }

    let sourceBuffers = UnsafeMutableAudioBufferListPointer(source.mutableAudioBufferList)
    guard sourceBuffers.count == expectedBufferCount else {
        throw MonoAudioBufferConversionError.bufferCountMismatch(
            expected: expectedBufferCount,
            actual: sourceBuffers.count
        )
    }
    for index in sourceBuffers.indices {
        let audioBuffer = sourceBuffers[index]
        guard Int(audioBuffer.mNumberChannels) == expectedChannelsPerBuffer else {
            throw MonoAudioBufferConversionError.bufferChannelCountMismatch(
                index: index,
                expected: expectedChannelsPerBuffer,
                actual: Int(audioBuffer.mNumberChannels)
            )
        }
        guard audioBuffer.mData != nil else {
            throw MonoAudioBufferConversionError.missingSourceData(index: index)
        }
        let actualByteCount = Int(audioBuffer.mDataByteSize)
        guard actualByteCount >= byteCount.partialValue else {
            throw MonoAudioBufferConversionError.sourceDataTooShort(
                index: index,
                expected: byteCount.partialValue,
                actual: actualByteCount
            )
        }
    }

    guard let monoFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: source.format.sampleRate,
        channels: 1,
        interleaved: false
    ), let monoBuffer = AVAudioPCMBuffer(
        pcmFormat: monoFormat,
        frameCapacity: source.frameLength
    ) else {
        throw MonoAudioBufferConversionError.allocationFailed
    }
    monoBuffer.frameLength = source.frameLength
    guard let monoData = monoBuffer.floatChannelData?[0] else {
        throw MonoAudioBufferConversionError.missingDestinationData
    }

    func sampleValue(
        from audioBuffer: AudioBuffer,
        bufferIndex: Int,
        sampleIndex: Int
    ) throws -> Double {
        guard let data = audioBuffer.mData else {
            throw MonoAudioBufferConversionError.missingSourceData(index: bufferIndex)
        }
        switch source.format.commonFormat {
        case .pcmFormatFloat32:
            return Double(data.assumingMemoryBound(to: Float.self)[sampleIndex])
        case .pcmFormatFloat64:
            return data.assumingMemoryBound(to: Double.self)[sampleIndex]
        case .pcmFormatInt16:
            return Double(data.assumingMemoryBound(to: Int16.self)[sampleIndex]) / 32_768.0
        case .pcmFormatInt32:
            return Double(data.assumingMemoryBound(to: Int32.self)[sampleIndex]) / 2_147_483_648.0
        case .otherFormat:
            throw MonoAudioBufferConversionError.unsupportedCommonFormat
        @unknown default:
            throw MonoAudioBufferConversionError.unsupportedCommonFormat
        }
    }

    for frame in 0..<frameCount {
        var sum = 0.0
        for channel in 0..<channelCount {
            let bufferIndex = source.format.isInterleaved ? 0 : channel
            let sampleIndex = source.format.isInterleaved
                ? frame * channelCount + channel
                : frame
            sum += try sampleValue(
                from: sourceBuffers[bufferIndex],
                bufferIndex: bufferIndex,
                sampleIndex: sampleIndex
            )
        }
        monoData[frame] = Float(sum / Double(channelCount))
    }
    return monoBuffer
}

func commonFormatName(_ format: AVAudioCommonFormat) -> String {
    switch format {
    case .pcmFormatFloat32: return "float32"
    case .pcmFormatFloat64: return "float64"
    case .pcmFormatInt16: return "int16"
    case .pcmFormatInt32: return "int32"
    case .otherFormat: return "other"
    @unknown default: return "unknown(\(format.rawValue))"
    }
}

func audioFormatDescription(_ format: AVAudioFormat) -> String {
    "sampleRate=\(format.sampleRate) channels=\(format.channelCount) commonFormat=\(commonFormatName(format.commonFormat))"
}

func rawAudioStreamDescription(_ format: AVAudioFormat) -> String {
    let asbd = format.streamDescription.pointee
    return "sampleRate=\(asbd.mSampleRate) formatID=\(asbd.mFormatID) formatFlags=0x\(String(asbd.mFormatFlags, radix: 16)) bytesPerPacket=\(asbd.mBytesPerPacket) framesPerPacket=\(asbd.mFramesPerPacket) bytesPerFrame=\(asbd.mBytesPerFrame) channelsPerFrame=\(asbd.mChannelsPerFrame) bitsPerChannel=\(asbd.mBitsPerChannel)"
}

func audioBufferLayoutDescription(
    _ buffer: AVAudioPCMBuffer,
    format: AVAudioFormat
) -> String {
    let buffers = UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)
    let byteSizes = buffers
        .map { String($0.mDataByteSize) }
        .joined(separator: ",")
    return "rawASBD=\(rawAudioStreamDescription(format)) interleaved=\(format.isInterleaved) bufferCount=\(buffers.count) bytes=\(byteSizes)"
}

func audioDurationNanoseconds(for buffer: AVAudioPCMBuffer) -> UInt64? {
    let frameLength = UInt64(buffer.frameLength)
    let sampleRate = buffer.format.sampleRate
    guard frameLength > 0, sampleRate.isFinite, sampleRate > 0 else { return nil }
    let duration = (Double(frameLength) / sampleRate) * 1_000_000_000
    guard duration.isFinite, duration > 0, duration <= Double(UInt64.max) else {
        return nil
    }
    return UInt64(ceil(duration))
}

func audioVolume(from buffer: AVAudioPCMBuffer) -> AudioVolumeMeasurement? {
    let commonFormat = buffer.format.commonFormat
    let bytesPerSample: Int
    switch commonFormat {
    case .pcmFormatFloat32: bytesPerSample = MemoryLayout<Float>.size
    case .pcmFormatFloat64: bytesPerSample = MemoryLayout<Double>.size
    case .pcmFormatInt16: bytesPerSample = MemoryLayout<Int16>.size
    case .pcmFormatInt32: bytesPerSample = MemoryLayout<Int32>.size
    case .otherFormat: return nil
    @unknown default: return nil
    }

    var peak = 0.0
    var squareSum = 0.0
    var sampleCount: UInt64 = 0
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
                let value = Double(samples[index])
                guard value.isFinite else { continue }
                peak = max(peak, abs(value))
                squareSum += value * value
                sampleCount &+= 1
            }
        case .pcmFormatFloat64:
            let samples = data.assumingMemoryBound(to: Double.self)
            for index in 0..<count {
                let value = samples[index]
                guard value.isFinite else { continue }
                peak = max(peak, abs(value))
                squareSum += value * value
                sampleCount &+= 1
            }
        case .pcmFormatInt16:
            let samples = data.assumingMemoryBound(to: Int16.self)
            for index in 0..<count {
                let value = Double(samples[index]) / 32_768.0
                peak = max(peak, abs(value))
                squareSum += value * value
                sampleCount &+= 1
            }
        case .pcmFormatInt32:
            let samples = data.assumingMemoryBound(to: Int32.self)
            for index in 0..<count {
                let value = Double(samples[index]) / 2_147_483_648.0
                peak = max(peak, abs(value))
                squareSum += value * value
                sampleCount &+= 1
            }
        case .otherFormat:
            return nil
        @unknown default:
            return nil
        }
    }
    guard sampleCount > 0 else { return nil }
    return AudioVolumeMeasurement(
        peak: peak,
        rms: (squareSum / Double(sampleCount)).squareRoot(),
        sampleCount: sampleCount
    )
}
