import AVFoundation
import Foundation

enum AppendedAudioDumpError: LocalizedError {
    case invalidDirectory
    case closed
    case sourceClosed(AudioSource)
    case formatMismatch(source: AudioSource, generation: Int)
    case writeFailed(path: String, details: String)

    var errorDescription: String? {
        switch self {
        case .invalidDirectory:
            return "追加音声ダンプの出力先ディレクトリが不正です"
        case .closed:
            return "追加音声ダンプはすでに終了しています"
        case let .sourceClosed(source):
            return "追加音声ダンプの source はすでに終了しています: \(source.rawValue)"
        case let .formatMismatch(source, generation):
            return "追加音声ダンプのフォーマットが区間内で変化しました: source=\(source.rawValue) generation=\(generation)"
        case let .writeFailed(path, details):
            return "追加音声ダンプを書き込めませんでした: path=\(path) \(details)"
        }
    }
}

final class AppendedAudioDump {
    private struct SegmentKey: Hashable {
        let source: AudioSource
        let generation: Int
    }

    private final class SegmentFile {
        let file: AVAudioFile
        let format: AVAudioFormat

        init(file: AVAudioFile, format: AVAudioFormat) {
            self.file = file
            self.format = format
        }
    }

    private let directoryURL: URL
    private let lock = NSLock()
    private var files: [SegmentKey: SegmentFile] = [:]
    private var closedSources: Set<AudioSource> = []
    private var closed = false

    init(directoryURL: URL) throws {
        guard !directoryURL.path.isEmpty else {
            throw AppendedAudioDumpError.invalidDirectory
        }
        do {
            try FileManager.default.createDirectory(
                at: directoryURL,
                withIntermediateDirectories: true
            )
        } catch {
            throw AppendedAudioDumpError.writeFailed(
                path: directoryURL.path,
                details: "directory=\(String(describing: error))"
            )
        }
        self.directoryURL = directoryURL
    }

    func append(
        _ buffer: AVAudioPCMBuffer,
        source: AudioSource,
        generation: Int
    ) throws {
        let key = SegmentKey(source: source, generation: generation)
        lock.lock()
        defer { lock.unlock() }
        guard !closed else { throw AppendedAudioDumpError.closed }
        guard !closedSources.contains(source) else {
            throw AppendedAudioDumpError.sourceClosed(source)
        }

        let segment: SegmentFile
        if let existing = files[key] {
            guard hasSameFormat(existing.format, buffer.format) else {
                throw AppendedAudioDumpError.formatMismatch(
                    source: source,
                    generation: generation
                )
            }
            segment = existing
        } else {
            do {
                let file = try AVAudioFile(
                    forWriting: url(source: source, generation: generation),
                    settings: buffer.format.settings,
                    commonFormat: buffer.format.commonFormat,
                    interleaved: buffer.format.isInterleaved
                )
                segment = SegmentFile(file: file, format: buffer.format)
                files[key] = segment
            } catch {
                throw AppendedAudioDumpError.writeFailed(
                    path: url(source: source, generation: generation).path,
                    details: "create=\(String(describing: error))"
                )
            }
        }

        do {
            try segment.file.write(from: buffer)
        } catch {
            throw AppendedAudioDumpError.writeFailed(
                path: url(source: source, generation: generation).path,
                details: "append=\(String(describing: error))"
            )
        }
    }

    func url(source: AudioSource, generation: Int) -> URL {
        directoryURL.appendingPathComponent(
            "segment-\(source.rawValue)-\(generation).wav",
            isDirectory: false
        )
    }

    func close(source: AudioSource, generation: Int) {
        lock.lock()
        files.removeValue(forKey: SegmentKey(source: source, generation: generation))
        lock.unlock()
    }

    func close(source: AudioSource) {
        lock.lock()
        closedSources.insert(source)
        files = files.filter { $0.key.source != source }
        lock.unlock()
    }

    func close() {
        lock.lock()
        closed = true
        files.removeAll()
        lock.unlock()
    }

    private func hasSameFormat(_ lhs: AVAudioFormat, _ rhs: AVAudioFormat) -> Bool {
        lhs.sampleRate == rhs.sampleRate
            && lhs.channelCount == rhs.channelCount
            && lhs.commonFormat == rhs.commonFormat
            && lhs.isInterleaved == rhs.isInterleaved
    }
}
