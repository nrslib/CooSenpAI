import AVFoundation

protocol AudioBufferAppendTarget {
    func append(_ buffer: AVAudioPCMBuffer, rms: Double)
}

enum ReceivedAudioBufferProcessingError: Error {
    case copy(AudioBufferCopyError)
    case normalization(MonoAudioBufferConversionError)
    case volumeUnavailable(AVAudioFormat)
}

struct ReceivedAudioBufferProcessingResult {
    let receivedFormat: AVAudioFormat
    let buffer: AVAudioPCMBuffer
    let rawVolume: AudioVolumeMeasurement
    let clampSummary: AudioSampleClampSummary
    let clampedVolume: AudioVolumeMeasurement?
}

enum ReceivedAudioBufferProcessor {
    static func processReceivedAudioBuffer(
        _ source: AVAudioPCMBuffer,
        appendTo target: AudioBufferAppendTarget
    ) throws -> ReceivedAudioBufferProcessingResult {
        let ownedBuffer: AVAudioPCMBuffer
        do {
            ownedBuffer = try deepCopyAudioBuffer(source)
        } catch let error as AudioBufferCopyError {
            throw ReceivedAudioBufferProcessingError.copy(error)
        }

        let normalizedBuffer: AVAudioPCMBuffer
        do {
            normalizedBuffer = try normalizedAudioBufferForAppend(from: ownedBuffer)
        } catch let error as MonoAudioBufferConversionError {
            throw ReceivedAudioBufferProcessingError.normalization(error)
        }

        guard let rawVolume = audioVolume(from: normalizedBuffer) else {
            throw ReceivedAudioBufferProcessingError.volumeUnavailable(normalizedBuffer.format)
        }
        let clampSummary = clampAudioSamples(in: normalizedBuffer)
        let clampedVolume = audioVolume(from: normalizedBuffer)
        if let clampedVolume {
            target.append(normalizedBuffer, rms: clampedVolume.rms)
        }
        return ReceivedAudioBufferProcessingResult(
            receivedFormat: ownedBuffer.format,
            buffer: normalizedBuffer,
            rawVolume: rawVolume,
            clampSummary: clampSummary,
            clampedVolume: clampedVolume
        )
    }
}
