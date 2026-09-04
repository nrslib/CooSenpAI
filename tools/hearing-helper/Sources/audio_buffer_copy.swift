import AVFoundation

enum AudioBufferCopyError: LocalizedError {
    case emptyBuffer
    case allocationFailed
    case bufferCountMismatch(expected: Int, actual: Int)
    case channelCountMismatch(index: Int, expected: Int, actual: Int)
    case missingSourceData(index: Int)
    case missingDestinationData(index: Int)
    case sourceDataTooShort(index: Int, expected: Int, actual: Int)
    case destinationDataTooShort(index: Int, expected: Int, actual: Int)

    var errorDescription: String? {
        switch self {
        case .emptyBuffer:
            return "音声バッファにフレームがありません"
        case .allocationFailed:
            return "音声バッファの複製先を確保できません"
        case let .bufferCountMismatch(expected, actual):
            return "音声バッファの数が一致しません: expected=\(expected) actual=\(actual)"
        case let .channelCountMismatch(index, expected, actual):
            return "音声バッファのチャンネル数が一致しません: index=\(index) expected=\(expected) actual=\(actual)"
        case let .missingSourceData(index):
            return "音声バッファの複製元データがありません: index=\(index)"
        case let .missingDestinationData(index):
            return "音声バッファの複製先データがありません: index=\(index)"
        case let .sourceDataTooShort(index, expected, actual):
            return "音声バッファの複製元データが短すぎます: index=\(index) expectedBytes=\(expected) actualBytes=\(actual)"
        case let .destinationDataTooShort(index, expected, actual):
            return "音声バッファの複製先データが短すぎます: index=\(index) expectedBytes=\(expected) actualBytes=\(actual)"
        }
    }
}

func deepCopyAudioBuffer(_ source: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
    let frameLength = source.frameLength
    guard frameLength > 0 else {
        throw AudioBufferCopyError.emptyBuffer
    }
    guard let destination = AVAudioPCMBuffer(
        pcmFormat: source.format,
        frameCapacity: frameLength
    ) else {
        throw AudioBufferCopyError.allocationFailed
    }
    destination.frameLength = frameLength

    let sourceBuffers = UnsafeMutableAudioBufferListPointer(source.mutableAudioBufferList)
    let destinationBuffers = UnsafeMutableAudioBufferListPointer(
        destination.mutableAudioBufferList
    )
    guard sourceBuffers.count == destinationBuffers.count else {
        throw AudioBufferCopyError.bufferCountMismatch(
            expected: destinationBuffers.count,
            actual: sourceBuffers.count
        )
    }

    for index in sourceBuffers.indices {
        let sourceBuffer = sourceBuffers[index]
        let destinationBuffer = destinationBuffers[index]
        guard sourceBuffer.mNumberChannels == destinationBuffer.mNumberChannels else {
            throw AudioBufferCopyError.channelCountMismatch(
                index: index,
                expected: Int(destinationBuffer.mNumberChannels),
                actual: Int(sourceBuffer.mNumberChannels)
            )
        }
        guard let sourceData = sourceBuffer.mData else {
            throw AudioBufferCopyError.missingSourceData(index: index)
        }
        guard let destinationData = destinationBuffer.mData else {
            throw AudioBufferCopyError.missingDestinationData(index: index)
        }

        let expectedByteCount = Int(destinationBuffer.mDataByteSize)
        let sourceByteCount = Int(sourceBuffer.mDataByteSize)
        guard expectedByteCount > 0 else {
            throw AudioBufferCopyError.destinationDataTooShort(
                index: index,
                expected: 1,
                actual: expectedByteCount
            )
        }
        guard sourceByteCount >= expectedByteCount else {
            throw AudioBufferCopyError.sourceDataTooShort(
                index: index,
                expected: expectedByteCount,
                actual: sourceByteCount
            )
        }
        destinationData.copyMemory(
            from: UnsafeRawPointer(sourceData),
            byteCount: expectedByteCount
        )
    }
    return destination
}
