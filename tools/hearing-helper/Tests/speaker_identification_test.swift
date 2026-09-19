import Foundation
import CryptoKit

final class InMemorySpeakerKeyStore: SpeakerKeyStore {
    private var key: SymmetricKey?

    func readExisting() throws -> SymmetricKey? {
        key
    }

    func readOrCreate() throws -> SymmetricKey {
        if let key { return key }
        let key = SymmetricKey(size: .bits256)
        self.key = key
        return key
    }

    func delete() throws {
        key = nil
    }
}

private let speakerTestModelPackageDigest = String(repeating: "a", count: 64)

final class BlockingSpeakerEmbeddingPredictor: SpeakerEmbeddingPredictor {
    let started = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let finished = DispatchSemaphore(value: 0)
    private let embedding = (0..<256).map { $0 == 0 ? Float(1) : Float(0) }

    func predict(features: [[Float]]) throws -> [Float] {
        started.signal()
        _ = release.wait(timeout: .now() + .seconds(5))
        finished.signal()
        return embedding
    }
}

final class ImmediateSpeakerEmbeddingPredictor: SpeakerEmbeddingPredictor {
    private let embedding = (0..<256).map { $0 == 0 ? Float(1) : Float(0) }
    private let countLock = NSLock()
    private var count = 0
    var predictionCount: Int {
        countLock.lock()
        defer { countLock.unlock() }
        return count
    }

    func predict(features: [[Float]]) throws -> [Float] {
        countLock.lock()
        count += 1
        countLock.unlock()
        return embedding
    }
}

private func waitForPredictions(_ predictor: ImmediateSpeakerEmbeddingPredictor, minimum: Int) {
    for _ in 0..<50 where predictor.predictionCount < minimum {
        Thread.sleep(forTimeInterval: 0.1)
    }
    assert(
        predictor.predictionCount >= minimum,
        "predictionCount=\(predictor.predictionCount), minimum=\(minimum)"
    )
    Thread.sleep(forTimeInterval: 0.1)
}

private func makeSpeakerIdentificationTestBuffer() -> AVAudioPCMBuffer {
    let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!
    let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 96_720)!
    buffer.frameLength = 96_720
    let samples = buffer.floatChannelData![0]
    for index in 0..<Int(buffer.frameLength) {
        samples[index] = 0.1
    }
    return buffer
}

private struct SpeakerFbankGoldenReference: Decodable {
    let schemaVersion: Int
    let preprocessingVersion: String
    let sampleRate: Int
    let channels: Int
    let sampleCount: Int
    let wavSha256: String
    let pcm16: [Int]
    let features: [[Float]]
    let normalizedEmbedding: [Float]
}

private func appendLittleEndian<T: FixedWidthInteger>(_ value: T, to data: inout Data) {
    var value = value.littleEndian
    withUnsafeBytes(of: &value) { bytes in
        data.append(contentsOf: bytes)
    }
}

private func speakerGoldenWavData(
    samples: [Int],
    sampleRate: Int
) -> Data {
    var data = Data()
    data.append(contentsOf: Array("RIFF".utf8))
    appendLittleEndian(UInt32(36 + samples.count * 2), to: &data)
    data.append(contentsOf: Array("WAVE".utf8))
    data.append(contentsOf: Array("fmt ".utf8))
    appendLittleEndian(UInt32(16), to: &data)
    appendLittleEndian(UInt16(1), to: &data)
    appendLittleEndian(UInt16(1), to: &data)
    appendLittleEndian(UInt32(sampleRate), to: &data)
    appendLittleEndian(UInt32(sampleRate * 2), to: &data)
    appendLittleEndian(UInt16(2), to: &data)
    appendLittleEndian(UInt16(16), to: &data)
    data.append(contentsOf: Array("data".utf8))
    appendLittleEndian(UInt32(samples.count * 2), to: &data)
    for sample in samples {
        appendLittleEndian(UInt16(bitPattern: Int16(sample)), to: &data)
    }
    return data
}

private func checkSpeakerFbankGoldenReference(
    at referenceURL: URL,
    modelCacheDirectory: URL?
) {
    let data = try! Data(contentsOf: referenceURL)
    let reference = try! JSONDecoder().decode(SpeakerFbankGoldenReference.self, from: data)
    assert(reference.schemaVersion == 2)
    assert(reference.preprocessingVersion == speakerIdentificationPreprocessingVersion)
    assert([16_000, 22_050, 44_100, 48_000].contains(reference.sampleRate))
    assert(reference.channels == 1)
    assert(reference.sampleCount == Int((2.015 * Double(reference.sampleRate)).rounded()))
    assert(reference.pcm16.count == reference.sampleCount)
    let wavData = speakerGoldenWavData(
        samples: reference.pcm16,
        sampleRate: reference.sampleRate
    )
    let wavDigest = SHA256.hash(data: wavData)
        .map { String(format: "%02x", $0) }
        .joined()
    assert(wavDigest == reference.wavSha256)

    let sourceSamples = reference.pcm16.map { Float($0) / 32_768.0 }
    let samples = try! speakerResample(sourceSamples, from: Double(reference.sampleRate))
    assert(samples.count == 32_240)
    let actual = try! WeSpeakerFbank.features(for: samples)
    assert(actual.count == reference.features.count)
    var maximumDifference: Float = 0
    for (actualFrame, expectedFrame) in zip(actual, reference.features) {
        assert(actualFrame.count == expectedFrame.count)
        for (actualValue, expectedValue) in zip(actualFrame, expectedFrame) {
            maximumDifference = max(maximumDifference, abs(actualValue - expectedValue))
        }
    }
    FileHandle.standardError.write(
        Data(
            ("Speaker fbank golden sample-rate=\(reference.sampleRate) "
                + "max-abs-error=\(maximumDifference)\n").utf8
        )
    )
    assert(
        maximumDifference <= 0.0005,
        "公式 torchaudio fbank との差が大きすぎます: \(maximumDifference)"
    )

    if let modelPath = ProcessInfo.processInfo.environment["COOSENPAI_SPEAKER_GOLDEN_MODEL"],
       !modelPath.isEmpty {
        let predictor = try! CoreMLSpeakerEmbeddingPredictor(
            path: modelPath,
            cacheDirectory: modelCacheDirectory
        )
        let embedding = try! predictor.predict(features: actual)
        assert(embedding.count == reference.normalizedEmbedding.count)
        let embeddingDifference = zip(embedding, reference.normalizedEmbedding)
            .map { abs($0 - $1) }
            .max() ?? .infinity
        assert(
            embeddingDifference <= 0.002,
            "公式 ONNX 埋め込みとの差が大きすぎます: \(embeddingDifference)"
        )
        FileHandle.standardError.write(
            Data(
                ("Speaker embedding golden sample-rate=\(reference.sampleRate) "
                    + "max-abs-error=\(embeddingDifference)\n").utf8
            )
        )
    }
}

private func testSpeakerFbankGoldenReference() {
    let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    var modelCacheDirectory: URL?
    var modelCacheCleanupDirectory: URL? = nil
    if ProcessInfo.processInfo.environment["COOSENPAI_SPEAKER_GOLDEN_MODEL"] != nil {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("coosenpai-speaker-golden-cache-\(UUID().uuidString)", isDirectory: true)
        try! FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        modelCacheDirectory = directory
        modelCacheCleanupDirectory = directory
    } else {
        modelCacheDirectory = nil
    }
    defer {
        if let modelCacheCleanupDirectory {
            try? FileManager.default.removeItem(at: modelCacheCleanupDirectory)
        }
    }
    checkSpeakerFbankGoldenReference(
        at: directory.appendingPathComponent("speaker-fbank-golden.json"),
        modelCacheDirectory: modelCacheDirectory
    )
    checkSpeakerFbankGoldenReference(
        at: directory.appendingPathComponent("speaker-fbank-golden-48k.json"),
        modelCacheDirectory: modelCacheDirectory
    )
    checkSpeakerFbankGoldenReference(
        at: directory.appendingPathComponent("speaker-fbank-golden-22050.json"),
        modelCacheDirectory: modelCacheDirectory
    )
    checkSpeakerFbankGoldenReference(
        at: directory.appendingPathComponent("speaker-fbank-golden-44100.json"),
        modelCacheDirectory: modelCacheDirectory
    )
    if let modelPath = ProcessInfo.processInfo.environment["COOSENPAI_SPEAKER_GOLDEN_MODEL"],
       !modelPath.isEmpty,
       let fixtureDirectory = ProcessInfo.processInfo.environment["COOSENPAI_SPEAKER_GOLDEN_FIXTURE_DIR"],
       !fixtureDirectory.isEmpty {
        testSpeakerGoldenSeparation(
            modelPath: modelPath,
            fixtureDirectory: URL(fileURLWithPath: fixtureDirectory),
            modelCacheDirectory: modelCacheDirectory
        )
    }
}

private func readSpeakerGoldenFixture(at url: URL) -> (samples: [Float], sampleRate: Double) {
    let audioFile = try! AVAudioFile(forReading: url)
    let buffer = AVAudioPCMBuffer(
        pcmFormat: audioFile.processingFormat,
        frameCapacity: AVAudioFrameCount(audioFile.length)
    )!
    try! audioFile.read(into: buffer)
    let mono = try! monoFloat32AudioBuffer(from: buffer)
    let samples = Array(
        UnsafeBufferPointer(
            start: mono.floatChannelData![0],
            count: Int(mono.frameLength)
        )
    )
    return (samples, mono.format.sampleRate)
}

private func normalizeSpeakerGoldenEmbedding(_ embedding: [Float]) -> [Float] {
    let norm = sqrt(embedding.reduce(Float.zero) { $0 + $1 * $1 })
    assert(norm.isFinite && norm > 0)
    return embedding.map { $0 / norm }
}

private func speakerGoldenSegmentEmbedding(
    samples: [Float],
    sampleRate: Double,
    predictor: CoreMLSpeakerEmbeddingPredictor
) -> [Float] {
    let windowSampleCount = Int((2.015 * sampleRate).rounded())
    let intervalSampleCount = Int(sampleRate.rounded())
    var embeddings: [[Float]] = []
    var start = 0
    while start <= samples.count - windowSampleCount {
        let source = Array(samples[start..<(start + windowSampleCount)])
        let window = try! speakerResample(source, from: sampleRate)
        let features = try! WeSpeakerFbank.features(for: window)
        embeddings.append(try! predictor.predict(features: features))
        start += intervalSampleCount
    }
    assert(!embeddings.isEmpty)
    let average = (0..<256).map { index in
        embeddings.reduce(Float.zero) { $0 + $1[index] } / Float(embeddings.count)
    }
    return normalizeSpeakerGoldenEmbedding(average)
}

private func speakerGoldenCosine(_ left: [Float], _ right: [Float]) -> Float {
    zip(left, right).reduce(Float.zero) { $0 + $1.0 * $1.1 }
}

private func testSpeakerGoldenSeparation(
    modelPath: String,
    fixtureDirectory: URL,
    modelCacheDirectory: URL?
) {
    let predictor = try! CoreMLSpeakerEmbeddingPredictor(
        path: modelPath,
        cacheDirectory: modelCacheDirectory
    )
    let first = readSpeakerGoldenFixture(
        at: fixtureDirectory.appendingPathComponent("speaker-a.wav")
    )
    let second = readSpeakerGoldenFixture(
        at: fixtureDirectory.appendingPathComponent("speaker-b.wav")
    )
    assert(first.sampleRate == 16_000 && second.sampleRate == 16_000)
    let firstEmbedding = speakerGoldenSegmentEmbedding(
        samples: first.samples,
        sampleRate: first.sampleRate,
        predictor: predictor
    )
    let secondEmbedding = speakerGoldenSegmentEmbedding(
        samples: second.samples,
        sampleRate: second.sampleRate,
        predictor: predictor
    )
    let cosine = speakerGoldenCosine(firstEmbedding, secondEmbedding)
    FileHandle.standardError.write(
        Data("Speaker separation golden cross-cosine=\(cosine)\n".utf8)
    )
    assert(cosine < speakerKnownSimilarityThreshold)
}

private func testSpeakerResampleNormalizesInputWindow() {
    for sampleRate in [22_050.0, 44_100.0] {
        let sourceCount = Int((2.015 * sampleRate).rounded())
        let source = Array(repeating: Float.zero, count: sourceCount)
        let resampled = try! speakerResample(source, from: sampleRate)
        assert(
            resampled.count == 32_240,
            "\(sampleRate) Hz のリサンプル後のサンプル数が不正です: \(resampled.count)"
        )
    }
}

private func testCoordinatorDeadlineDoesNotCommit() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-deadline-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let keyStore = InMemorySpeakerKeyStore()
    let predictor = BlockingSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    assert(predictor.started.wait(timeout: .now() + .seconds(5)) == .success)

    let result = coordinator.finishSegment(
        generation: 1,
        audioEndNanoseconds: 2_010_000_000
    )
    assert(result.status == .unavailable, "期限中の結果が想定外です: \(result.status)")
    predictor.release.signal()
    assert(predictor.finished.wait(timeout: .now() + .seconds(5)) == .success)
    Thread.sleep(forTimeInterval: 0.1)
    assert(!FileManager.default.fileExists(atPath: ledgerPath))
    assert(!FileManager.default.fileExists(atPath: root.appendingPathComponent("aliases.json").path))
    assert((try! keyStore.readExisting()) == nil)
    coordinator.shutdown()
    try? FileManager.default.removeItem(at: root)
}

private func testCoordinatorCancelsReservedPersistence() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-persistence-deadline-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let keyStore = InMemorySpeakerKeyStore()
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceDelay: 0.75
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let startedAt = DispatchTime.now().uptimeNanoseconds
    let result = coordinator.finishSegment(
        generation: 1,
        audioEndNanoseconds: 4_010_000_000
    )
    let elapsed = Double(DispatchTime.now().uptimeNanoseconds - startedAt) / 1_000_000_000
    assert(result.status == .unavailable, "予約後の結果が想定外です: \(result.status)")
    assert(elapsed < 0.70, "予約後の遅延で音声処理キューを塞いでいます: \(elapsed)秒")
    Thread.sleep(forTimeInterval: 2.0)
    assert(predictor.predictionCount >= 2, "predictionCount=\(predictor.predictionCount)")
    assert(!FileManager.default.fileExists(atPath: ledgerPath))
    assert(!FileManager.default.fileExists(atPath: root.appendingPathComponent("aliases.json").path))
    assert((try! keyStore.readExisting()) == nil)
    coordinator.shutdown()
    try? FileManager.default.removeItem(at: root)
}

private func testPersistenceAfterBeginDoesNotBlockAudioQueue() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-persistence-audio-queue-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let persistenceBegan = DispatchSemaphore(value: 0)
    let nextAudioProcessed = DispatchSemaphore(value: 0)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceDelayAfterBegin: 0.75,
        persistenceBegan: { persistenceBegan.signal() }
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let audioQueue = DispatchQueue(label: "coosenpai-speaker-test-audio-queue")
    audioQueue.async {
        let result = coordinator.finishSegment(
            generation: 1,
            audioEndNanoseconds: 4_010_000_000
        )
        assert(result.status == .unavailable)
    }
    assert(
        persistenceBegan.wait(timeout: .now() + .seconds(5)) == .success,
        "shutdown persistence callback did not run; predictionCount=\(predictor.predictionCount)"
    )
    let nextAudioQueuedAt = DispatchTime.now().uptimeNanoseconds
    audioQueue.async {
        coordinator.beginSegment(
            generation: 2,
            sampleRate: 48_000,
            audioStartNanoseconds: 5_000_000_000
        )
        coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 2)
        nextAudioProcessed.signal()
    }
    assert(nextAudioProcessed.wait(timeout: .now() + .seconds(2)) == .success)
    let nextAudioElapsed = Double(
        DispatchTime.now().uptimeNanoseconds - nextAudioQueuedAt
    ) / 1_000_000_000
    assert(nextAudioElapsed < 1.25, "永続化中に後続音声処理が止まりました: \(nextAudioElapsed)秒")
    coordinator.shutdown()
    try? FileManager.default.removeItem(at: root)
}

private func testPersistenceCompletionKeepsFollowingEmbeddingIdentity() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-persistence-follow-up-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let keyStore = InMemorySpeakerKeyStore()
    let persistenceBegan = DispatchSemaphore(value: 0)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceDelayAfterBegin: 0.75,
        persistenceBegan: { persistenceBegan.signal() }
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let first = coordinator.finishSegment(
        generation: 1,
        audioEndNanoseconds: 4_010_000_000
    )
    assert(first.status == .unavailable)
    assert(persistenceBegan.wait(timeout: .now() + .seconds(5)) == .success)

    let persistenceDeadline = Date().addingTimeInterval(5)
    while !FileManager.default.fileExists(atPath: ledgerPath) && Date() < persistenceDeadline {
        Thread.sleep(forTimeInterval: 0.1)
    }
    assert(FileManager.default.fileExists(atPath: ledgerPath), "遅延永続化が完了していません")

    coordinator.beginSegment(
        generation: 2,
        sampleRate: 48_000,
        audioStartNanoseconds: 5_000_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 2)
    waitForPredictions(predictor, minimum: 4)
    let second = coordinator.finishSegment(
        generation: 2,
        audioEndNanoseconds: 7_010_000_000
    )
    assert(second.status == .identified)
    assert(second.speakerID == "speaker-1")
    assert(second.registryID != nil)

    let restored = try! SpeakerLedger(
        path: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let embedding = (0..<256).map { $0 == 0 ? Float(1) : Float(0) }
    let restoredResult = try! restored.identify(windows: [
        SpeakerEmbeddingWindow(embedding: embedding, startSample: 0, weight: 1)
    ])
    assert(restoredResult.0 == "speaker-1")
    assert(restoredResult.1 == second.registryID)
    coordinator.shutdown()
}

private func testCoordinatorCancelsAfterReservationBeforePersistence() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-cancel-barrier-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let reserved = DispatchSemaphore(value: 0)
    let finished = DispatchSemaphore(value: 0)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceDelay: 0.75,
        persistenceReserved: { reserved.signal() },
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let finishQueue = DispatchQueue(label: "coosenpai-speaker-cancel-barrier-finish")
    finishQueue.async {
        let result = coordinator.finishSegment(
            generation: 1,
            audioEndNanoseconds: 4_010_000_000
        )
        assert(result.status == .unavailable)
        finished.signal()
    }
    assert(
        reserved.wait(timeout: .now() + .seconds(5)) == .success,
        "reservation callback did not run; predictionCount=\(predictor.predictionCount)"
    )
    coordinator.cancel()
    assert(finished.wait(timeout: .now() + .seconds(5)) == .success)
    coordinator.shutdown()
    assert(!FileManager.default.fileExists(atPath: ledgerPath))
    assert(!FileManager.default.fileExists(atPath: root.appendingPathComponent("aliases.json").path))
    try? FileManager.default.removeItem(at: root)
}

private func testCoordinatorInvalidatesReservationWhenNextGenerationStarts() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-generation-barrier-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let reserved = DispatchSemaphore(value: 0)
    let finished = DispatchSemaphore(value: 0)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceDelay: 0.75,
        persistenceReserved: { reserved.signal() }
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let finishQueue = DispatchQueue(label: "coosenpai-speaker-generation-barrier-finish")
    finishQueue.async {
        let result = coordinator.finishSegment(
            generation: 1,
            audioEndNanoseconds: 4_010_000_000
        )
        assert(result.status == .unavailable)
        finished.signal()
    }
    assert(
        reserved.wait(timeout: .now() + .seconds(5)) == .success,
        "generation reservation callback did not run; predictionCount=\(predictor.predictionCount)"
    )
    coordinator.beginSegment(
        generation: 2,
        sampleRate: 48_000,
        audioStartNanoseconds: 5_000_000_000
    )
    assert(finished.wait(timeout: .now() + .seconds(5)) == .success)
    coordinator.shutdown()
    assert(!FileManager.default.fileExists(atPath: ledgerPath))
    try? FileManager.default.removeItem(at: root)
}

private func testCoordinatorSingleWindowDoesNotRegister() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-single-window-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let keyStore = InMemorySpeakerKeyStore()
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 1)
    let result = coordinator.finishSegment(
        generation: 1,
        audioEndNanoseconds: 2_010_000_000
    )
    assert(result.status == .unknown, "単一有効窓を登録しました: \(result.status)")
    assert(!FileManager.default.fileExists(atPath: ledgerPath))
    assert((try! keyStore.readExisting()) == nil)
    coordinator.shutdown()
    try? FileManager.default.removeItem(at: root)
}

private func testCoordinatorShutdownWaitsForStartedPersistence() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-shutdown-barrier-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let persistenceBegan = DispatchSemaphore(value: 0)
    let releasePersistence = DispatchSemaphore(value: 0)
    let shutdownFinished = DispatchSemaphore(value: 0)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: ledgerPath,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest,
        persistenceBegan: {
            persistenceBegan.signal()
            _ = releasePersistence.wait(timeout: .now() + .seconds(5))
        }
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    waitForPredictions(predictor, minimum: 3)
    let finishQueue = DispatchQueue(label: "coosenpai-speaker-shutdown-barrier-finish")
    finishQueue.async {
        _ = coordinator.finishSegment(
            generation: 1,
            audioEndNanoseconds: 4_010_000_000
        )
    }
    assert(
        persistenceBegan.wait(timeout: .now() + .seconds(5)) == .success,
        "shutdown persistence callback did not run; predictionCount=\(predictor.predictionCount)"
    )
    let shutdownQueue = DispatchQueue(label: "coosenpai-speaker-shutdown-barrier-shutdown")
    shutdownQueue.async {
        coordinator.shutdown()
        shutdownFinished.signal()
    }
    assert(shutdownFinished.wait(timeout: .now() + .milliseconds(200)) == .timedOut)
    releasePersistence.signal()
    assert(shutdownFinished.wait(timeout: .now() + .seconds(5)) == .success)
    assert(FileManager.default.fileExists(atPath: ledgerPath))
    try? FileManager.default.removeItem(at: root)
}

private func testPreparingSegmentIsNotReprocessed() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-preparing-\(UUID().uuidString)", isDirectory: true)
    let predictor = ImmediateSpeakerEmbeddingPredictor()
    let coordinator = try! SpeakerIdentificationCoordinator(
        forTesting: predictor,
        ledgerPath: root.appendingPathComponent("registry.enc").path,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest,
        preparationState: .preparing
    )
    coordinator.beginSegment(
        generation: 1,
        sampleRate: 48_000,
        audioStartNanoseconds: 10_000_000
    )
    coordinator.append(makeSpeakerIdentificationTestBuffer(), generation: 1)
    let result = coordinator.finishSegment(
        generation: 1,
        audioEndNanoseconds: 2_010_000_000
    )
    assert(result.status == .unavailable)
    Thread.sleep(forTimeInterval: 0.1)
    assert(predictor.predictionCount == 0)
    coordinator.shutdown()
    try? FileManager.default.removeItem(at: root)
}

private func makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: Float) -> [Float] {
    let sine = sqrt(max(0, 1 - cosineWithFirstAxis * cosineWithFirstAxis))
    var embedding = Array(repeating: Float.zero, count: 256)
    embedding[0] = cosineWithFirstAxis
    embedding[1] = sine
    return embedding
}

private func testSpeakerWindowClusterDecision() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-cluster-\(UUID().uuidString)")
    let ledger = try! SpeakerLedger(
        path: root.appendingPathComponent("registry.enc").path,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let firstSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let secondSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.05)
    func windows(firstCount: Int, secondCount: Int) -> [SpeakerEmbeddingWindow] {
        (0..<(firstCount + secondCount)).map { index in
            SpeakerEmbeddingWindow(
                embedding: index < firstCount ? firstSpeaker : secondSpeaker,
                startSample: index * 32_240,
                weight: 1
            )
        }
    }

    let dominant = try! ledger.diagnose(windows: windows(firstCount: 4, secondCount: 1))
    assert(dominant.windowCount == 5)
    assert(dominant.primaryClusterCount == 4)
    assert(dominant.secondaryClusterCount == 1)
    assert(dominant.status == .identified)

    let twoLargeClusters = try! ledger.diagnose(
        windows: windows(firstCount: 3, secondCount: 2)
    )
    assert(twoLargeClusters.primaryClusterCount == 3)
    assert(twoLargeClusters.secondaryClusterCount == 2)
    assert(twoLargeClusters.status == .mixed)
    try? FileManager.default.removeItem(at: root)
}

private func testSpeakerDiagnosisLedgerRegistration() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-diagnosis-ledger-\(UUID().uuidString)")
    let legacyLedger = try! SpeakerLedger(
        forDiagnosisAt: root.appendingPathComponent("legacy/registry.enc").path,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let currentLedger = try! SpeakerLedger(
        forDiagnosisAt: root.appendingPathComponent("current/registry.enc").path,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let firstSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let secondSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.05)
    func windows() -> [SpeakerEmbeddingWindow] {
        (0..<5).map { index in
            SpeakerEmbeddingWindow(
                embedding: index < 4 ? firstSpeaker : secondSpeaker,
                startSample: index * 32_240,
                weight: 1
            )
        }
    }

    let legacy = try! legacyLedger.identifyForDiagnosis(
        windows: windows(),
        generation: 1,
        rule: .allPairwise
    )
    assert(legacy.diagnostic.status == .mixed)
    assert(legacy.speakerID == nil)
    assert(legacyLedger.activeProfileCount == 0)

    let current = try! currentLedger.identifyForDiagnosis(
        windows: windows(),
        generation: 1,
        rule: .dominantCluster
    )
    assert(current.diagnostic.status == .identified)
    assert(current.speakerID == "speaker-1")
    assert(currentLedger.activeProfileCount == 1)

    let repeated = try! currentLedger.identifyForDiagnosis(
        windows: windows(),
        generation: 2,
        rule: .dominantCluster
    )
    assert(repeated.speakerID == "speaker-1")
    assert(currentLedger.activeProfileCount == 1)
    try? FileManager.default.removeItem(at: root)
}

private func testSpeakerDiagnosisNewThresholdOverrideEnrolls() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-new-threshold-\(UUID().uuidString)")
    let ledger = try! SpeakerLedger(
        forDiagnosisAt: root.appendingPathComponent("registry.enc").path,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let firstSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let borderlineSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.28)
    func windows(_ embedding: [Float]) -> [SpeakerEmbeddingWindow] {
        (0..<3).map { index in
            SpeakerEmbeddingWindow(
                embedding: embedding,
                startSample: index * 32_240,
                weight: 1
            )
        }
    }

    let registered = try! ledger.identifyForDiagnosis(
        windows: windows(firstSpeaker),
        generation: 1,
        rule: .dominantCluster
    )
    assert(registered.speakerID == "speaker-1")

    let defaultResult = try! ledger.identifyForDiagnosis(
        windows: windows(borderlineSpeaker),
        generation: 2,
        rule: .dominantCluster
    )
    assert(defaultResult.speakerID == nil)
    assert(defaultResult.diagnostic.status == .unknown)
    assert(ledger.activeProfileCount == 1)

    let relaxedRule = SpeakerDecisionRule.dominantCluster.applying(
        thresholdOverride: SpeakerDiagnosisThresholdOverride(
            knownSimilarityThreshold: nil,
            newSpeakerSimilarityThreshold: 0.30,
            windowConsistencyThreshold: nil
        )
    )
    let overridden = try! ledger.identifyForDiagnosis(
        windows: windows(borderlineSpeaker),
        generation: 3,
        rule: relaxedRule
    )
    assert(overridden.speakerID == "speaker-2")
    assert(overridden.diagnostic.status == .identified)
    assert(ledger.activeProfileCount == 2)
    try? FileManager.default.removeItem(at: root)
}

private func testSpeakerDiagnosisKnownThresholdOverrideRejectsMatch() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-known-threshold-\(UUID().uuidString)")
    let ledger = try! SpeakerLedger(
        forDiagnosisAt: root.appendingPathComponent("registry.enc").path,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let firstSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let closeSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.50)
    func windows(_ embedding: [Float]) -> [SpeakerEmbeddingWindow] {
        (0..<3).map { index in
            SpeakerEmbeddingWindow(
                embedding: embedding,
                startSample: index * 32_240,
                weight: 1
            )
        }
    }

    let registered = try! ledger.identifyForDiagnosis(
        windows: windows(firstSpeaker),
        generation: 1,
        rule: .dominantCluster
    )
    assert(registered.speakerID == "speaker-1")

    let defaultResult = try! ledger.identifyForDiagnosis(
        windows: windows(closeSpeaker),
        generation: 2,
        rule: .dominantCluster
    )
    assert(defaultResult.speakerID == "speaker-1")
    assert(defaultResult.diagnostic.status == .identified)

    let stricterRule = SpeakerDecisionRule.dominantCluster.applying(
        thresholdOverride: SpeakerDiagnosisThresholdOverride(
            knownSimilarityThreshold: 0.60,
            newSpeakerSimilarityThreshold: nil,
            windowConsistencyThreshold: nil
        )
    )
    let overridden = try! ledger.identifyForDiagnosis(
        windows: windows(closeSpeaker),
        generation: 3,
        rule: stricterRule
    )
    assert(overridden.speakerID == nil)
    assert(overridden.diagnostic.status == .unknown)
    assert(ledger.activeProfileCount == 1)
    try? FileManager.default.removeItem(at: root)
}

private func testSpeakerDiagnosisWindowThresholdOverrideAffectsMixed() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-window-threshold-\(UUID().uuidString)")
    let ledger = try! SpeakerLedger(
        forDiagnosisAt: root.appendingPathComponent("registry.enc").path,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    // 互いの cosine が 0.09 の窓集合: 既定 0.10 では同一クラスタにならず、0.05 では一つにまとまる
    let windows = (0..<5).map { index in
        var embedding = Array(repeating: Float.zero, count: 256)
        embedding[0] = 0.3
        embedding[index + 1] = sqrt(Float(0.91))
        return SpeakerEmbeddingWindow(
            embedding: embedding,
            startSample: index * 32_240,
            weight: 1
        )
    }

    let defaultResult = try! ledger.identifyForDiagnosis(
        windows: windows,
        generation: 1,
        rule: .dominantCluster
    )
    assert(defaultResult.diagnostic.status == .mixed)
    assert(ledger.activeProfileCount == 0)

    let relaxedRule = SpeakerDecisionRule.dominantCluster.applying(
        thresholdOverride: SpeakerDiagnosisThresholdOverride(
            knownSimilarityThreshold: nil,
            newSpeakerSimilarityThreshold: nil,
            windowConsistencyThreshold: 0.05
        )
    )
    let overridden = try! ledger.identifyForDiagnosis(
        windows: windows,
        generation: 2,
        rule: relaxedRule
    )
    assert(overridden.diagnostic.status == .identified)
    assert(overridden.speakerID == "speaker-1")
    assert(ledger.activeProfileCount == 1)
    try? FileManager.default.removeItem(at: root)
}

private func assertSpeakerDiagnosisThresholdParseFails(
    _ arguments: [String],
    reason: String
) {
    switch parseSpeakerDiagnosisThresholdOverride(arguments: arguments) {
    case .success:
        assert(false, "不正な引数を受理しました (\(reason)): \(arguments)")
    case .failure:
        break
    }
}

private func testSpeakerDiagnosisThresholdArgumentParsing() {
    switch parseSpeakerDiagnosisThresholdOverride(arguments: [
        "--speaker-diagnose", "dump",
        "--speaker-known-threshold", "0.40",
        "--speaker-new-threshold", "0.25",
        "--speaker-window-threshold", "0.10",
    ]) {
    case .success(let parsed):
        assert(parsed.remainingArguments == ["--speaker-diagnose", "dump"])
        assert(parsed.override.hasOverride)
        assert(parsed.override.knownSimilarityThreshold == 0.40)
        assert(parsed.override.newSpeakerSimilarityThreshold == 0.25)
        assert(parsed.override.windowConsistencyThreshold == 0.10)
    case .failure(let error):
        assert(false, "正常な引数を拒否しました: \(error.message)")
    }

    switch parseSpeakerDiagnosisThresholdOverride(arguments: [
        "--speaker-model", "model.mlpackage",
    ]) {
    case .success(let parsed):
        assert(parsed.remainingArguments == ["--speaker-model", "model.mlpackage"])
        assert(!parsed.override.hasOverride)
        assert(parsed.override.knownSimilarityThreshold == nil)
        assert(parsed.override.newSpeakerSimilarityThreshold == nil)
        assert(parsed.override.windowConsistencyThreshold == nil)
    case .failure(let error):
        assert(false, "しきい値なしの引数を拒否しました: \(error.message)")
    }

    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-known-threshold"],
        reason: "値なし"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-known-threshold", "abc"],
        reason: "非数値"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-known-threshold", "1.5"],
        reason: "範囲外"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-window-threshold", "-1.5"],
        reason: "範囲外"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-new-threshold", "0.1", "--speaker-new-threshold", "0.2"],
        reason: "重複指定"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-known-threshold", "0.2", "--speaker-new-threshold", "0.3"],
        reason: "new > known"
    )
    assertSpeakerDiagnosisThresholdParseFails(
        ["--speaker-new-threshold", "0.5"],
        reason: "既定の known を超える new"
    )
}

private func testPreviousDecisionVersionLedgerIsRejected() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-decision-version-\(UUID().uuidString)")
    let ledgerURL = root.appendingPathComponent("registry.enc")
    let keyStore = InMemorySpeakerKeyStore()
    let ledger = try! SpeakerLedger(
        path: ledgerURL.path,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let embedding = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let windows = (0..<2).map { index in
        SpeakerEmbeddingWindow(
            embedding: embedding,
            startSample: index * 32_240,
            weight: 1
        )
    }
    assert(try! ledger.identify(windows: windows).0 == "speaker-1")

    // 保存時は現行の判定版が書かれている。ここから判定版だけを v4 へ書き換える
    let key = try! keyStore.readExisting()!
    let envelope = try! JSONSerialization.jsonObject(
        with: try! Data(contentsOf: ledgerURL)
    ) as! [String: Any]
    let combined = Data(base64Encoded: envelope["combined"] as! String)!
    let plain = try! AES.GCM.open(try! AES.GCM.SealedBox(combined: combined), using: key)
    var document = try! JSONSerialization.jsonObject(with: plain) as! [String: Any]
    assert(document["decisionVersion"] as? String == speakerIdentificationDecisionVersion)
    document["decisionVersion"] = "speaker-cosine-ledger-v4"
    let sealed = try! AES.GCM.seal(
        try! JSONSerialization.data(withJSONObject: document),
        using: key
    )
    let rewrittenEnvelope: [String: Any] = [
        "schemaVersion": 1,
        "combined": sealed.combined!.base64EncodedString(),
    ]
    try! JSONSerialization.data(withJSONObject: rewrittenEnvelope).write(to: ledgerURL)

    var rejected = false
    do {
        _ = try SpeakerLedger(
            path: ledgerURL.path,
            keyStore: keyStore,
            modelPackageDigest: speakerTestModelPackageDigest
        )
    } catch let error as SpeakerIdentificationFailure {
        if case .decisionVersionMismatch = error {
            rejected = error.localizedDescription.contains("再登録")
        }
    } catch {}
    assert(rejected, "旧判定版の台帳を判定版不一致として拒否しませんでした")
    // 無効な台帳は空の台帳へ置き換えず残す
    assert(FileManager.default.fileExists(atPath: ledgerURL.path))

    // 台帳と鍵を削除すれば、新しい判定版で最初から登録し直せる
    try! FileManager.default.removeItem(at: ledgerURL)
    try! keyStore.delete()
    let fresh = try! SpeakerLedger(
        path: ledgerURL.path,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    assert(try! fresh.identify(windows: windows).0 == "speaker-1")
    try? FileManager.default.removeItem(at: root)
}

private func testCentroidAutoUpdateDisabledUntilCalibration() {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-centroid-confidence-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: root) }
    let ledger = try! SpeakerLedger(
        path: root.appendingPathComponent("registry.enc").path,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let firstSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 1)
    let lowConfidenceOtherSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.40)
    let secondSpeaker = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0)
    let differentSpeakerHighScoreObservation = makeSpeakerClusterTestEmbedding(cosineWithFirstAxis: 0.80)

    func windows(_ embedding: [Float], count: Int) -> [SpeakerEmbeddingWindow] {
        (0..<count).map { index in
            SpeakerEmbeddingWindow(
                embedding: embedding,
                startSample: index * 32_240,
                weight: 1
            )
        }
    }

    assert(try! ledger.identify(windows: windows(firstSpeaker, count: 2)).0 == "speaker-1")
    let beforeLowConfidenceMatch = try! ledger.diagnose(
        windows: [SpeakerEmbeddingWindow(embedding: lowConfidenceOtherSpeaker, startSample: 0, weight: 1)]
    ).candidateScores.first!.score
    let lowConfidenceResult = try! ledger.identify(
        windows: windows(lowConfidenceOtherSpeaker, count: 3)
    )
    assert(lowConfidenceResult.0 == "speaker-1")
    assert(lowConfidenceResult.2 == .identified)
    let afterLowConfidenceMatch = try! ledger.diagnose(
        windows: [SpeakerEmbeddingWindow(embedding: lowConfidenceOtherSpeaker, startSample: 0, weight: 1)]
    ).candidateScores.first!.score
    assert(
        abs(afterLowConfidenceMatch - beforeLowConfidenceMatch) < 0.0001,
        "低信頼一致で centroid が更新されました: before=\(beforeLowConfidenceMatch), after=\(afterLowConfidenceMatch)"
    )

    assert(try! ledger.identify(windows: windows(secondSpeaker, count: 2)).0 == "speaker-2")
    let ledgerURL = root.appendingPathComponent("registry.enc")
    let ledgerBeforeHighConfidenceMatch = try! Data(contentsOf: ledgerURL)
    let beforeHighConfidenceMatch = try! ledger.diagnose(
        windows: [SpeakerEmbeddingWindow(embedding: differentSpeakerHighScoreObservation, startSample: 0, weight: 1)]
    ).candidateScores.first!.score
    let highConfidenceCandidates = try! ledger.diagnose(
        windows: windows(differentSpeakerHighScoreObservation, count: 3)
    ).candidateScores
    assert(highConfidenceCandidates.count == 2)
    assert(highConfidenceCandidates[0].score >= 0.55)
    assert(highConfidenceCandidates[0].score - highConfidenceCandidates[1].score >= 0.15)
    let highConfidenceResult = try! ledger.identify(
        windows: windows(differentSpeakerHighScoreObservation, count: 3)
    )
    assert(highConfidenceResult.0 == "speaker-1")
    assert(highConfidenceResult.2 == .identified)
    let afterHighConfidenceMatch = try! ledger.diagnose(
        windows: [SpeakerEmbeddingWindow(embedding: differentSpeakerHighScoreObservation, startSample: 0, weight: 1)]
    ).candidateScores.first!.score
    assert(
        abs(afterHighConfidenceMatch - beforeHighConfidenceMatch) < 0.0001,
        "高スコア・十分な margin の一致で anchor/centroid が変化しました: before=\(beforeHighConfidenceMatch), after=\(afterHighConfidenceMatch)"
    )
    assert(
        (try! Data(contentsOf: ledgerURL)) == ledgerBeforeHighConfidenceMatch,
        "校正前の centroid 自動更新で台帳が変化しました"
    )
}

func testSpeakerIdentification() {
    let identifiedFields = speakerEventFields(
        SpeakerIdentificationResult(
            segmentID: "segment-1",
            audioStartMilliseconds: 1_000,
            audioEndMilliseconds: 3_000,
            speakerID: "speaker-1",
            registryID: "registry-1",
            status: .identified
        )
    )
    assert(
        Set(identifiedFields.keys) == [
            "segmentId",
            "audioStartMs",
            "audioEndMs",
            "speakerStatus",
            "speakerId",
            "speakerRegistryId",
        ]
    )
    assert(identifiedFields["speakerId"] as? String == "speaker-1")
    assert(identifiedFields["speakerRegistryId"] as? String == "registry-1")
    for forbiddenKey in [
        "embedding",
        "centroid",
        "anchor",
        "fbank",
        "pcm",
        "audio",
        "featureVector",
    ] {
        assert(identifiedFields[forbiddenKey] == nil)
    }
    let unknownFields = speakerEventFields(
        SpeakerIdentificationResult(
            segmentID: "segment-2",
            audioStartMilliseconds: 3_000,
            audioEndMilliseconds: 5_000,
            speakerID: "speaker-2",
            registryID: "registry-1",
            status: .unknown
        )
    )
    assert(unknownFields["speakerId"] == nil)
    assert(unknownFields["speakerRegistryId"] == nil)

    testSpeakerFbankGoldenReference()
    testSpeakerResampleNormalizesInputWindow()
    testSpeakerWindowClusterDecision()
    testSpeakerDiagnosisLedgerRegistration()
    testSpeakerDiagnosisNewThresholdOverrideEnrolls()
    testSpeakerDiagnosisKnownThresholdOverrideRejectsMatch()
    testSpeakerDiagnosisWindowThresholdOverrideAffectsMixed()
    testSpeakerDiagnosisThresholdArgumentParsing()
    testPreviousDecisionVersionLedgerIsRejected()
    testCentroidAutoUpdateDisabledUntilCalibration()

    let window = Array(repeating: Float.zero, count: 32_240)
    let features = try! WeSpeakerFbank.features(for: window)
    assert(features.count == 200)
    assert(features.allSatisfy { $0.count == 80 })
    let sineWindow = (0..<32_240).map { index in
        let time = Double(index) / 16_000.0
        return Float(
            0.2 * sin(2.0 * Double.pi * 440.0 * time)
                + 0.1 * sin(2.0 * Double.pi * 880.0 * time)
        )
    }
    let sineFeatures = try! WeSpeakerFbank.features(for: sineWindow)
    assert(sineFeatures.allSatisfy { $0.allSatisfy { $0.isFinite } })

    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-test-\(UUID().uuidString)", isDirectory: true)
    let ledgerPath = root.appendingPathComponent("registry.enc").path
    let keyStore = InMemorySpeakerKeyStore()
    let ledger = try! SpeakerLedger(
        path: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let embedding: [Float] = (0..<256).map { $0 == 0 ? 1 : 0 }
    let windows = [
        SpeakerEmbeddingWindow(embedding: embedding, startSample: 0, weight: 1),
        SpeakerEmbeddingWindow(embedding: embedding, startSample: 32_240, weight: 1),
    ]
    let first = try! ledger.identify(windows: windows)
    assert(first.0 == "speaker-1")
    assert(first.2 == .identified)
    assert(!FileManager.default.fileExists(atPath: ledgerPath + ".lock"))
    let ledgerAttributes = try! FileManager.default.attributesOfItem(atPath: ledgerPath)
    assert((ledgerAttributes[.posixPermissions] as! NSNumber).intValue & 0o777 == 0o600)
    let aliasAttributes = try! FileManager.default.attributesOfItem(
        atPath: root.appendingPathComponent("aliases.json").path
    )
    assert((aliasAttributes[.posixPermissions] as! NSNumber).intValue & 0o777 == 0o600)
    let directoryAttributes = try! FileManager.default.attributesOfItem(atPath: root.path)
    assert((directoryAttributes[.posixPermissions] as! NSNumber).intValue & 0o777 == 0o700)
    let restored = try! SpeakerLedger(
        path: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let matched = try! restored.identify(windows: [windows[0]])
    assert(matched.0 == "speaker-1")
    var modelMismatchRejected = false
    do {
        _ = try SpeakerLedger(
            path: ledgerPath,
            keyStore: keyStore,
            modelPackageDigest: String(repeating: "b", count: 64)
        )
    } catch let error as SpeakerIdentificationFailure {
        modelMismatchRejected = error.localizedDescription.contains("異なります")
    } catch {}
    assert(modelMismatchRejected)

    let candidatePath = root.appendingPathComponent("candidate-registry.enc").path
    let candidateKeyStore = InMemorySpeakerKeyStore()
    let candidateLedger = try! SpeakerLedger(
        path: candidatePath,
        keyStore: candidateKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let remembered = try! candidateLedger.identify(windows: [windows[0]])
    assert(remembered.0 == nil)
    assert(remembered.2 == .unknown)
    let enrolled = try! candidateLedger.identify(windows: windows)
    assert(enrolled.0 == "speaker-1")

    let deadlinePath = root.appendingPathComponent("deadline-registry.enc").path
    let deadlineKeyStore = InMemorySpeakerKeyStore()
    let deadlineLedger = try! SpeakerLedger(
        path: deadlinePath,
        keyStore: deadlineKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    var commitChecks = 0
    var deadlineRejected = false
    do {
        _ = try deadlineLedger.identify(windows: windows, canCommit: {
            commitChecks += 1
            return commitChecks == 1
        })
    } catch let error as SpeakerIdentificationFailure {
        deadlineRejected = error.localizedDescription.contains("期限")
    } catch {}
    assert(deadlineRejected)
    assert(commitChecks >= 2)
    assert(!FileManager.default.fileExists(atPath: deadlinePath))
    assert((try! deadlineKeyStore.readExisting()) == nil)

    try! restored.reregister(id: "speaker-1")
    let second = try! restored.identify(windows: windows)
    assert(second.0 == "speaker-2")
    try! restored.deleteAll()
    let afterDelete = try! SpeakerLedger(
        path: ledgerPath,
        keyStore: keyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let afterDeleteRegistration = try! afterDelete.identify(windows: windows)
    assert(afterDeleteRegistration.0 == "speaker-3")
    assert(!FileManager.default.fileExists(atPath: ledgerPath + ".lock"))

    let aliasRoot = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-alias-rebuild-\(UUID().uuidString)", isDirectory: true)
    let aliasLedgerPath = aliasRoot.appendingPathComponent("registry.enc").path
    let aliasKeyStore = InMemorySpeakerKeyStore()
    let aliasLedger = try! SpeakerLedger(
        path: aliasLedgerPath,
        keyStore: aliasKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let otherEmbedding: [Float] = (0..<256).map { $0 == 1 ? 1 : 0 }
    let otherWindows = [
        SpeakerEmbeddingWindow(embedding: otherEmbedding, startSample: 0, weight: 1),
        SpeakerEmbeddingWindow(embedding: otherEmbedding, startSample: 32_240, weight: 1),
    ]
    assert((try! aliasLedger.identify(windows: windows)).0 == "speaker-1")
    assert((try! aliasLedger.identify(windows: otherWindows)).0 == "speaker-2")
    var failAliasWrite = false
    let failingAliasLedger = try! SpeakerLedger(
        path: aliasLedgerPath,
        keyStore: aliasKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest,
        shouldFailAliasWrite: { failAliasWrite }
    )
    failAliasWrite = true
    var aliasWriteFailed = false
    do {
        try failingAliasLedger.merge(from: "speaker-1", to: "speaker-2")
    } catch let error as SpeakerIdentificationFailure {
        if case .ledgerAliasWrite = error { aliasWriteFailed = true }
    } catch {}
    assert(aliasWriteFailed)
    var identificationStopped = false
    do {
        _ = try failingAliasLedger.identify(windows: windows)
    } catch let error as SpeakerIdentificationFailure {
        if case .ledgerAliasWrite = error { identificationStopped = true }
    } catch {}
    assert(identificationStopped)
    _ = try! SpeakerLedger(
        path: aliasLedgerPath,
        keyStore: aliasKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    let rebuiltAliasData = try! Data(
        contentsOf: aliasRoot.appendingPathComponent("aliases.json")
    )
    let rebuiltAliasObject = try! JSONSerialization.jsonObject(with: rebuiltAliasData) as! [String: Any]
    let rebuiltAliases = rebuiltAliasObject["aliases"] as! [String: String]
    assert(rebuiltAliases["speaker-1"] == "speaker-2")
    try? FileManager.default.removeItem(at: aliasRoot)

    let missingPath = root.appendingPathComponent("missing-registry.enc")
    let missingKeyStore = InMemorySpeakerKeyStore()
    let missingLedger = try! SpeakerLedger(
        path: missingPath.path,
        keyStore: missingKeyStore,
        modelPackageDigest: speakerTestModelPackageDigest
    )
    _ = try! missingLedger.identify(windows: windows)
    try! FileManager.default.removeItem(at: missingPath)
    var missingFileRejected = false
    do {
        _ = try SpeakerLedger(path: missingPath.path, keyStore: missingKeyStore)
    } catch let error as SpeakerIdentificationFailure {
        missingFileRejected = error.localizedDescription.contains("鍵だけ")
    } catch {}
    assert(missingFileRejected)
    assert((try! missingKeyStore.readExisting()) != nil)

    let stalePath = root.appendingPathComponent("stale-registry.enc")
    let staleLockPath = stalePath.appendingPathExtension("lock")
    FileManager.default.createFile(
        atPath: staleLockPath.path,
        contents: Data("2147483647 0\n".utf8)
    )
    let staleLedger = try! SpeakerLedger(
        path: stalePath.path,
        keyStore: InMemorySpeakerKeyStore(),
        modelPackageDigest: speakerTestModelPackageDigest
    )
    assert(!FileManager.default.fileExists(atPath: staleLockPath.path))
    _ = try! staleLedger.identify(windows: windows)
    let malformedStaleLockPath = root.appendingPathComponent("malformed-stale-registry.enc.lock")
    FileManager.default.createFile(
        atPath: malformedStaleLockPath.path,
        contents: Data("incomplete".utf8)
    )
    try! FileManager.default.setAttributes(
        [.modificationDate: Date(timeIntervalSinceNow: -300)],
        ofItemAtPath: malformedStaleLockPath.path
    )
    _ = try! SpeakerLedger(
        path: root.appendingPathComponent("malformed-stale-registry.enc").path,
        keyStore: InMemorySpeakerKeyStore()
    )
    assert(!FileManager.default.fileExists(atPath: malformedStaleLockPath.path))
    let activeLockPath = root.appendingPathComponent("active-registry.enc.lock")
    FileManager.default.createFile(
        atPath: activeLockPath.path,
        contents: Data("\(getpid()) 0\n".utf8)
    )
    var activeLockRejected = false
    do {
        _ = try SpeakerLedger(
            path: root.appendingPathComponent("active-registry.enc").path,
            keyStore: InMemorySpeakerKeyStore()
        )
    } catch let error as SpeakerIdentificationFailure {
        activeLockRejected = error.localizedDescription.contains("別の処理")
    } catch {}
    assert(activeLockRejected)
    try! FileManager.default.removeItem(at: activeLockPath)

    var missingModelRejected = false
    do {
        _ = try SpeakerIdentificationCoordinator(
            modelPath: nil,
            ledgerPath: ledgerPath,
            log: { _ in }
        )
    } catch let error as SpeakerIdentificationFailure {
        missingModelRejected = error.localizedDescription.contains("モデル")
    } catch {}
    assert(missingModelRejected)

    var missingModelFileRejected = false
    do {
        _ = try SpeakerIdentificationCoordinator(
            modelPath: root.appendingPathComponent("missing-model.mlpackage").path,
            ledgerPath: ledgerPath,
            log: { _ in }
        )
    } catch let error as SpeakerIdentificationFailure {
        missingModelFileRejected = error.localizedDescription.contains("モデル")
    } catch {}
    assert(missingModelFileRejected)

    let directCompiledModelPath = root.appendingPathComponent("direct.mlmodelc")
    try! FileManager.default.createDirectory(at: directCompiledModelPath, withIntermediateDirectories: true)
    var directCompiledModelRejected = false
    do {
        _ = try CoreMLSpeakerEmbeddingPredictor(path: directCompiledModelPath.path)
    } catch let error as SpeakerIdentificationFailure {
        directCompiledModelRejected = error.localizedDescription.contains(".mlpackage")
    } catch {}
    assert(directCompiledModelRejected)
    try? FileManager.default.removeItem(at: directCompiledModelPath)

    try! keyStore.delete()
    var missingKeyRejected = false
    do {
        _ = try SpeakerLedger(path: ledgerPath, keyStore: keyStore)
    } catch let error as SpeakerIdentificationFailure {
        missingKeyRejected = error.localizedDescription.contains("鍵")
    } catch {}
    assert(missingKeyRejected)
    let orphanAliasRoot = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-speaker-orphan-alias-\(UUID().uuidString)", isDirectory: true)
    try! FileManager.default.createDirectory(at: orphanAliasRoot, withIntermediateDirectories: true)
    let orphanAliasPath = orphanAliasRoot.appendingPathComponent("aliases.json")
    FileManager.default.createFile(atPath: orphanAliasPath.path, contents: Data("{}".utf8))
    let orphanLedger = try! SpeakerLedger(
        path: orphanAliasRoot.appendingPathComponent("registry.enc").path,
        keyStore: InMemorySpeakerKeyStore()
    )
    try! orphanLedger.deleteAll()
    assert(!FileManager.default.fileExists(atPath: orphanAliasPath.path))
    try? FileManager.default.removeItem(at: orphanAliasRoot)
    testCoordinatorDeadlineDoesNotCommit()
    testCoordinatorCancelsReservedPersistence()
    testPersistenceAfterBeginDoesNotBlockAudioQueue()
    testPersistenceCompletionKeepsFollowingEmbeddingIdentity()
    testCoordinatorCancelsAfterReservationBeforePersistence()
    testCoordinatorInvalidatesReservationWhenNextGenerationStarts()
    testCoordinatorSingleWindowDoesNotRegister()
    testCoordinatorShutdownWaitsForStartedPersistence()
    testPreparingSegmentIsNotReprocessed()
    try! candidateLedger.deleteAll()
    try! FileManager.default.removeItem(at: root)
}
