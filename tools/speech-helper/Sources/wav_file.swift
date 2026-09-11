import AudioToolbox
import AVFoundation
import Darwin

// 排他作成した fd を所有し、ヘッダーの確定までパスを再解決しない。
final class SpeechWavFile {
    private var descriptor: Int32
    private var audioFile: AudioFileID?
    private var extendedFile: ExtAudioFileRef?

    init(descriptor: Int32, format: AVAudioFormat) throws {
        self.descriptor = descriptor
        guard fchmod(descriptor, mode_t(0o600)) == 0,
              let fileFormat = AVAudioFormat(
                commonFormat: format.commonFormat, sampleRate: format.sampleRate,
                channels: format.channelCount, interleaved: true
              ) else {
            _ = dispose()
            throw SpeechWavDumpError.createFailed
        }
        let context = Unmanaged.passUnretained(self).toOpaque()
        guard AudioFileInitializeWithCallbacks(
            context, Self.read, Self.write, Self.size, Self.resize,
            kAudioFileWAVEType, fileFormat.streamDescription, [], &audioFile
        ) == noErr, let audioFile,
              ExtAudioFileWrapAudioFileID(audioFile, true, &extendedFile) == noErr,
              let extendedFile,
              ExtAudioFileSetProperty(
                extendedFile, kExtAudioFileProperty_ClientDataFormat,
                UInt32(MemoryLayout<AudioStreamBasicDescription>.size), format.streamDescription
              ) == noErr else {
            _ = dispose()
            throw SpeechWavDumpError.createFailed
        }
    }

    deinit { _ = dispose() }

    func write(_ buffer: AVAudioPCMBuffer) throws {
        guard let extendedFile else { throw SpeechWavDumpError.closed }
        guard ExtAudioFileWrite(extendedFile, buffer.frameLength, buffer.audioBufferList) == noErr else {
            throw SpeechWavDumpError.writeFailed
        }
    }

    func close() throws {
        guard dispose() else { throw SpeechWavDumpError.writeFailed }
    }

    private func dispose() -> Bool {
        var succeeded = true
        if let extendedFile {
            if ExtAudioFileDispose(extendedFile) != noErr { succeeded = false }
            self.extendedFile = nil
        }
        if let audioFile {
            if AudioFileClose(audioFile) != noErr { succeeded = false }
            self.audioFile = nil
        }
        if descriptor >= 0 {
            if Darwin.close(descriptor) != 0 { succeeded = false }
            descriptor = -1
        }
        return succeeded
    }

    private static let read: AudioFile_ReadProc = { context, position, count, buffer, actual in
        let file = Unmanaged<SpeechWavFile>.fromOpaque(context).takeUnretainedValue()
        actual.pointee = 0
        while actual.pointee < count {
            let offset = Int(actual.pointee)
            let read = pread(file.descriptor, buffer.advanced(by: offset), Int(count) - offset, position + Int64(offset))
            if read < 0 {
                if errno == EINTR { continue }
                return kAudioFileUnspecifiedError
            }
            if read == 0 { break }
            actual.pointee += UInt32(read)
        }
        return noErr
    }

    private static let write: AudioFile_WriteProc = { context, position, count, buffer, actual in
        let file = Unmanaged<SpeechWavFile>.fromOpaque(context).takeUnretainedValue()
        actual.pointee = 0
        while actual.pointee < count {
            let offset = Int(actual.pointee)
            let written = pwrite(file.descriptor, buffer.advanced(by: offset), Int(count) - offset, position + Int64(offset))
            if written < 0 {
                if errno == EINTR { continue }
                return kAudioFileUnspecifiedError
            }
            if written == 0 { return kAudioFileUnspecifiedError }
            actual.pointee += UInt32(written)
        }
        return noErr
    }

    private static let size: AudioFile_GetSizeProc = { context in
        let file = Unmanaged<SpeechWavFile>.fromOpaque(context).takeUnretainedValue()
        var attributes = stat()
        guard fstat(file.descriptor, &attributes) == 0 else { return -1 }
        return attributes.st_size
    }

    private static let resize: AudioFile_SetSizeProc = { context, size in
        let file = Unmanaged<SpeechWavFile>.fromOpaque(context).takeUnretainedValue()
        return ftruncate(file.descriptor, size) == 0 ? noErr : kAudioFileUnspecifiedError
    }
}
