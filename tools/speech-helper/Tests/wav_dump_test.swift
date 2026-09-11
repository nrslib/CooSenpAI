import AVFoundation
import Darwin
import Foundation

func testWavDump() {
    runTest("排他作成の直後にパスが symlink へ置換されても元の fd に保存する") {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        let path = directory.appendingPathComponent("capture.wav")
        let moved = directory.appendingPathComponent("opened.wav")
        let victim = directory.appendingPathComponent("existing.wav")
        let original = Data([1, 2, 3, 4])
        try original.write(to: victim)
        let descriptor = open(path.path, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode_t(0o600))
        expect(descriptor >= 0, "保存先を排他作成する")
        do {
            try FileManager.default.moveItem(at: path, to: moved)
            try FileManager.default.createSymbolicLink(at: path, withDestinationURL: victim)
        } catch {
            Darwin.close(descriptor)
            throw error
        }
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        let file = try SpeechWavFile(descriptor: descriptor, format: format)
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 3)!
        buffer.frameLength = 3
        let expected: [Float] = [0.25, -0.5, 0.75]
        for index in expected.indices { buffer.floatChannelData![0][index] = expected[index] }
        try file.write(buffer)
        try file.close()
        expect(fcntl(descriptor, F_GETFD) == -1 && errno == EBADF, "WAV の確定後に fd を閉じる")
        let remaining = try Data(contentsOf: victim)
        expect(remaining == original, "symlink 先を上書きしない")
        let attributes = try FileManager.default.attributesOfItem(atPath: moved.path)
        expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600, "権限も元の fd に設定する")
        let saved = try AVAudioFile(forReading: moved)
        let result = AVAudioPCMBuffer(pcmFormat: saved.processingFormat, frameCapacity: 3)!
        try saved.read(into: result)
        expect(saved.length == 3 && result.frameLength == 3, "移動されたファイルのヘッダーを確定する")
        expect(Array(UnsafeBufferPointer(start: result.floatChannelData![0], count: 3)) == expected,
               "同じ fd へ全 PCM を書き込む")
    }

    runTest("WAV の初期化失敗でも引き受けた fd を解放する") {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString).wav")
        defer { try? FileManager.default.removeItem(at: path) }
        try Data([1, 2, 3, 4]).write(to: path)
        let descriptor = open(path.path, O_RDONLY | O_CLOEXEC)
        expect(descriptor >= 0, "読み取り専用 fd を作る")
        do {
            _ = try SpeechWavFile(descriptor: descriptor, format: AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!)
            expect(false, "書き込み不能なら初期化に失敗する")
        } catch SpeechWavDumpError.createFailed {}
        expect(fcntl(descriptor, F_GETFD) == -1 && errno == EBADF, "初期化失敗で fd を漏らさない")
    }

    for channels: AVAudioChannelCount in [1, 2] {
        runTest("マイク WAV 保存は \(channels) ch の PCM・順序・末尾を保持し再入力できる") {
            let url = FileManager.default.temporaryDirectory
                .appendingPathComponent("speech-dump-\(UUID().uuidString).wav")
            defer { try? FileManager.default.removeItem(at: url) }
            do {
                let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: channels)!
                let dump = try SpeechWavDump(path: url.path, format: format)
                let expected = (0..<Int(channels)).map { channel in
                    (0..<2_053).map { Float(($0 + channel * 32) % 256) / 256 - 0.5 }
                }
                var offset = 0
                for frameCount: AVAudioFrameCount in [1_024, 1_024, 5] {
                    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frameCount)!
                    buffer.frameLength = frameCount
                    for channel in 0..<Int(channels) {
                        for frame in 0..<Int(frameCount) {
                            buffer.floatChannelData![channel][frame] = expected[channel][offset + frame]
                        }
                    }
                    try dump.append(buffer)
                    for channel in 0..<Int(channels) {
                        for frame in 0..<Int(frameCount) { buffer.floatChannelData![channel][frame] = 0 }
                    }
                    offset += Int(frameCount)
                }
                let frames = try dump.close()
                expect(frames == 2_053, "終了時に未書き込みの全フレームを保存する")
                let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
                expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600, "保存音声を所有者だけが読める")
                let file = try AVAudioFile(forReading: url)
                expect(file.fileFormat.sampleRate == 48_000 && file.fileFormat.channelCount == channels,
                       "サンプルレートとチャンネル数を変換しない")
                expect(file.fileFormat.commonFormat == .pcmFormatFloat32, "元の PCM 精度を保持する")
                let player = try DebugInputWavPlayer(path: url.path, playbackRate: 1)
                var samples = Array(repeating: [Float](), count: Int(channels))
                var succeeded = false
                let completed = DispatchSemaphore(value: 0)
                player.start(
                    onBuffer: { buffer in
                        for channel in 0..<Int(channels) {
                            samples[channel] += Array(UnsafeBufferPointer(
                                start: buffer.floatChannelData![channel], count: Int(buffer.frameLength)
                            ))
                        }
                    },
                    onCompletion: { result in
                        if case .success = result { succeeded = true }
                        completed.signal()
                    }
                )
                expect(completed.wait(timeout: .now() + 2) == .success, "保存 WAV を最後まで再入力する")
                expect(succeeded && samples == expected, "tap の再利用で変化せず全サンプルが一致する")
                player.stop()
            } catch {
                expect(false, "WAV 保存テストに失敗しました: \(error)")
            }
        }
    }

    runTest("マイク WAV 保存は既存ファイルと symlink を上書きしない") {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
            let url = directory.appendingPathComponent("existing.wav")
            let link = directory.appendingPathComponent("link.wav")
            let original = Data([1, 2, 3, 4])
            try original.write(to: url)
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: url)
            let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
            for path in [url.path, link.path] {
                do {
                    _ = try SpeechWavDump(path: path, format: format)
                    expect(false, "既存の保存先を拒否する")
                } catch SpeechWavDumpError.existingFile {}
            }
            let remaining = try Data(contentsOf: url)
            expect(remaining == original, "元のファイル内容を維持する")
        } catch {
            expect(false, "上書き拒否テストに失敗しました: \(error)")
        }
    }

    runTest("マイク WAV 保存は出力先の不備を録音前に拒否する") {
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
        for path in ["", "/tmp/speech-dump-not-wav.txt"] {
            do {
                _ = try SpeechWavDump(path: path, format: format)
                expect(false, "WAV 以外の保存先を拒否する")
            } catch SpeechWavDumpError.invalidPath {
            } catch { expect(false, "保存先の形式を検証する: \(error)") }
        }
        let missingParent = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString)/missing.wav")
        do {
            _ = try SpeechWavDump(path: missingParent.path, format: format)
            expect(false, "親ディレクトリがない場合は失敗する")
        } catch SpeechWavDumpError.createFailed {
        } catch { expect(false, "保存先の作成失敗を返す: \(error)") }
        expect(!FileManager.default.fileExists(atPath: missingParent.path), "出力先を勝手に作り直さない")
    }

    runTest("マイク WAV 保存は空でも終了でき終了後の音声を保存しない") {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString).wav")
        defer { try? FileManager.default.removeItem(at: url) }
        do {
            let format = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!
            let dump = try SpeechWavDump(path: url.path, format: format)
            let frames = try dump.close()
            let closedAgain = try dump.close()
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
            buffer.frameLength = 1
            buffer.floatChannelData![0][0] = 0.5
            do {
                try dump.append(buffer)
                expect(false, "終了後の音声を拒否する")
            } catch SpeechWavDumpError.closed {}
            let file = try AVAudioFile(forReading: url)
            expect(frames == 0 && closedAgain == 0 && file.length == 0, "空の WAV を確定する")
            expect(file.fileFormat.sampleRate == 44_100, "48 kHz に固定しない")
        } catch {
            expect(false, "終了テストに失敗しました: \(error)")
        }
    }

    runTest("マイク WAV 保存は途中の PCM 形式変更を拒否する") {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("speech-dump-\(UUID().uuidString).wav")
        defer { try? FileManager.default.removeItem(at: url) }
        do {
            let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 1)!
            let dump = try SpeechWavDump(path: url.path, format: format)
            let other = AVAudioFormat(standardFormatWithSampleRate: 44_100, channels: 1)!
            let buffer = AVAudioPCMBuffer(pcmFormat: other, frameCapacity: 1)!
            buffer.frameLength = 1
            do {
                try dump.append(buffer)
                expect(false, "PCM 形式変更を拒否する")
            } catch SpeechWavDumpError.formatMismatch {}
            let frames = try dump.close()
            expect(frames == 0, "不一致のフレームを保存しない")
        } catch {
            expect(false, "PCM 形式テストに失敗しました: \(error)")
        }
    }
}
