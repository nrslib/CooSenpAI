import Accelerate
import AVFoundation
import CoreML
import CryptoKit
import Darwin
import Foundation
import Security

let speakerIdentificationModelIdentifier = "wespeaker-voxceleb-resnet34-LM"
let speakerIdentificationModelWeightSHA256 = "25ac42cda3fca8a8093d862da90aa3746c93110ea5e3058b292a5bf8df093821"
let speakerIdentificationPreprocessingVersion = "wespeaker-kaldi-fbank-snip-edges-true-200fr-2015ms-resample-trim-v4"
let speakerIdentificationDecisionVersion = "speaker-cosine-ledger-v5"

private let speakerIdentificationModelPackageFiles = [
    (path: "Manifest.json", sha256: "b8c0b0d83caef37e7fe6d4ad8006085c16e916145c1e705d6079eefdc1c78af7"),
    (path: "Data/com.apple.CoreML/model.mlmodel", sha256: "61155e50979e8aab98226f81ae24327428d70e390eb1733e7109d4fcf00f1962"),
    (path: "Data/com.apple.CoreML/weights/weight.bin", sha256: speakerIdentificationModelWeightSHA256),
]
private let speakerIdentificationCompiledModelCacheVersion = "coreml-compiled-v1"

private let speakerSampleRate = 16_000.0
private let speakerWindowSampleCount = 32_240
private let speakerEvidenceWindowSampleCount = 32_000
private let speakerWindowDurationSeconds = 2.015
private let speakerWindowFrameCount = 200
private let speakerFrameSampleCount = 400
private let speakerFrameShiftSampleCount = 160
private let speakerMelBinCount = 80
private let speakerMinimumSpeechSamples = 24_180
private let speakerLegacyKnownSimilarityThreshold: Float = 0.65
private let speakerLegacySingleWindowSimilarityThreshold: Float = 0.75
private let speakerLegacyNewSpeakerSimilarityThreshold: Float = 0.45
// 現行規則の既定値。known 0.35 は据え置き、new 0.25 と window 0.10 は
// 2026-09-18 の実動画校正（男性1人・女性3人、スピーカー区間265本、
// 正解ラベルなしの40通り総当たり）で採用した。
// 根拠は docs/plans/speaker-id-2026-09-16.md §4 を参照。
let speakerKnownSimilarityThreshold: Float = 0.35
private let speakerSingleWindowSimilarityThreshold: Float = 0.35
private let speakerNewSpeakerSimilarityThreshold: Float = 0.25
private let speakerLegacyWindowConsistencyThreshold: Float = 0.75
private let speakerWindowConsistencyThreshold: Float = 0.10
private let speakerWindowPrimaryClusterFraction = 0.60
private let speakerWindowSecondaryClusterFraction = 0.40
private let speakerMarginThreshold: Float = 0.10
private let speakerCentroidUpdateSimilarityThreshold: Float = 0.55
private let speakerCentroidUpdateMarginThreshold: Float = 0.15
private let speakerCentroidUpdateMinimumWindowCount = 3
// 正解ラベル付き校正が完了するまで、永続 centroid の自動更新は行わない。
private let speakerCentroidAutoUpdateEnabled = false
private let speakerMaximumProfiles = 1_000
private let speakerMaximumRegistryBytes = 16 * 1024 * 1024
private let speakerMaximumPendingCandidates = 32
private let speakerMaximumInvalidatedGenerations = 64
private let speakerPendingCandidateLifetimeSeconds = 60.0
private let speakerLedgerLockStaleSeconds = 120.0
private let speakerLedgerCommitSafetyNanoseconds: UInt64 = 50_000_000

enum SpeakerIdentificationFailure: LocalizedError {
    case modelPathMissing
    case modelFileMissing(String)
    case modelLoad(String)
    case modelPackageMismatch
    case modelInputUnavailable
    case modelOutputInvalid
    case ledgerPathMissing
    case ledgerLocked
    case ledgerMissing
    case ledgerCorrupt
    case decisionVersionMismatch
    case ledgerKeyUnavailable(OSStatus)
    case ledgerWrite(String)
    case ledgerAliasWrite(String)
    case invalidEmbedding
    case deadlineExceeded
    case modelPreparing
    case modelUnavailable
    case diagnosisInputMissing
    case diagnosisInputDirectoryUnreadable
    case diagnosisInputHasNoMatchingWav(String)
    case diagnosisInputNotWav
    case diagnosisInputRead(String)

    var stopsIdentification: Bool {
        if case .ledgerAliasWrite = self { return true }
        return false
    }

    var errorDescription: String? {
        switch self {
        case .modelPathMissing:
            return "話者識別モデルの場所が指定されていません"
        case let .modelFileMissing(path):
            return "話者識別モデルが見つかりません: \(path)"
        case let .modelLoad(details):
            return "話者識別モデルを読み込めません: \(details)"
        case .modelPackageMismatch:
            return "話者識別モデルが台帳作成時と異なります。照合を停止し、話者を再登録してください"
        case .modelInputUnavailable:
            return "話者識別モデルの入力仕様を利用できません"
        case .modelOutputInvalid:
            return "話者識別モデルの出力が不正です"
        case .ledgerPathMissing:
            return "話者台帳の保存先が指定されていません"
        case .ledgerLocked:
            return "話者台帳が別の処理で使用されています"
        case .ledgerMissing:
            return "話者台帳の本体がありません。鍵だけを残した状態では新しい台帳として開始しません"
        case .ledgerCorrupt:
            return "話者台帳を読み込めません"
        case .decisionVersionMismatch:
            return "話者台帳の判定版が台帳作成時と異なります。台帳を削除して話者を再登録してください"
        case let .ledgerKeyUnavailable(status):
            return "話者台帳の鍵を利用できません: status=\(status)"
        case let .ledgerWrite(details):
            return "話者台帳を保存できません: \(details)"
        case let .ledgerAliasWrite(details):
            return "話者台帳の別名索引を保存できません。識別を停止し、次回起動時に再構築します: \(details)"
        case .invalidEmbedding:
            return "話者埋め込みの値または次元が不正です"
        case .deadlineExceeded:
            return "話者識別の処理期限を超えました"
        case .modelPreparing:
            return "話者識別モデルを準備中です"
        case .modelUnavailable:
            return "話者識別モデルを利用できません"
        case .diagnosisInputMissing:
            return "話者識別の診断入力が見つかりません"
        case .diagnosisInputDirectoryUnreadable:
            return "話者識別の診断入力ディレクトリを読み込めません"
        case let .diagnosisInputHasNoMatchingWav(prefix):
            return "話者識別の診断入力ディレクトリに \(prefix) の WAV がありません"
        case .diagnosisInputNotWav:
            return "話者識別の診断入力には WAV ファイルまたは dump ディレクトリを指定してください"
        case let .diagnosisInputRead(details):
            return "話者識別の診断入力を読み込めません: \(details)"
        }
    }
}

struct SpeakerIdentificationResult {
    let segmentID: String
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
}

func speakerEventFields(_ result: SpeakerIdentificationResult) -> [String: Any] {
    var fields: [String: Any] = [
        "segmentId": result.segmentID,
        "audioStartMs": result.audioStartMilliseconds,
        "audioEndMs": result.audioEndMilliseconds,
        "speakerStatus": result.status.rawValue,
    ]
    if result.status == .identified, let speakerID = result.speakerID {
        fields["speakerId"] = speakerID
    }
    if result.status == .identified, let registryID = result.registryID {
        fields["speakerRegistryId"] = registryID
    }
    return fields
}

enum SpeakerIdentificationStatusValue: String, Codable {
    case identified
    case unknown
    case mixed
    case unavailable
}

enum SpeakerIdentificationPreparationStatus: String {
    case preparing
    case ready
    case unavailable
}

struct SpeakerEmbeddingWindow {
    let embedding: [Float]
    let startSample: Int
    let weight: Float
}

struct SpeakerIdentificationDiagnostic {
    let windowCount: Int
    let pairwiseSimilarities: [Float]
    let candidateScores: [(id: String, score: Float)]
    let primaryClusterCount: Int
    let secondaryClusterCount: Int
    let status: SpeakerIdentificationStatusValue
}

struct SpeakerDiagnosisThresholdOverride {
    let knownSimilarityThreshold: Float?
    let newSpeakerSimilarityThreshold: Float?
    let windowConsistencyThreshold: Float?

    var hasOverride: Bool {
        knownSimilarityThreshold != nil
            || newSpeakerSimilarityThreshold != nil
            || windowConsistencyThreshold != nil
    }
}

struct SpeakerDecisionRule {
    enum WindowClustering {
        case allPairwise
        case dominantCluster
    }

    let windowClustering: WindowClustering
    let windowConsistencyThreshold: Float
    let knownSimilarityThreshold: Float
    let singleWindowSimilarityThreshold: Float
    let newSpeakerSimilarityThreshold: Float
    let allowsSingleWindowEnrollment: Bool

    static let allPairwise = SpeakerDecisionRule(
        windowClustering: .allPairwise,
        windowConsistencyThreshold: speakerLegacyWindowConsistencyThreshold,
        knownSimilarityThreshold: speakerLegacyKnownSimilarityThreshold,
        singleWindowSimilarityThreshold: speakerLegacySingleWindowSimilarityThreshold,
        newSpeakerSimilarityThreshold: speakerLegacyNewSpeakerSimilarityThreshold,
        allowsSingleWindowEnrollment: true
    )

    static let dominantCluster = SpeakerDecisionRule(
        windowClustering: .dominantCluster,
        windowConsistencyThreshold: speakerWindowConsistencyThreshold,
        knownSimilarityThreshold: speakerKnownSimilarityThreshold,
        singleWindowSimilarityThreshold: speakerSingleWindowSimilarityThreshold,
        newSpeakerSimilarityThreshold: speakerNewSpeakerSimilarityThreshold,
        allowsSingleWindowEnrollment: false
    )

    func applying(thresholdOverride: SpeakerDiagnosisThresholdOverride) -> SpeakerDecisionRule {
        SpeakerDecisionRule(
            windowClustering: windowClustering,
            windowConsistencyThreshold: thresholdOverride.windowConsistencyThreshold
                ?? windowConsistencyThreshold,
            knownSimilarityThreshold: thresholdOverride.knownSimilarityThreshold
                ?? knownSimilarityThreshold,
            singleWindowSimilarityThreshold: singleWindowSimilarityThreshold,
            newSpeakerSimilarityThreshold: thresholdOverride.newSpeakerSimilarityThreshold
                ?? newSpeakerSimilarityThreshold,
            allowsSingleWindowEnrollment: allowsSingleWindowEnrollment
        )
    }
}

struct SpeakerDiagnosisIdentification {
    let diagnostic: SpeakerIdentificationDiagnostic
    let speakerID: String?
    let registryID: String?
    let segmentEmbedding: [Float]?
}

private struct SpeakerSegment {
    let generation: Int
    let segmentID: String
    let audioStartNanoseconds: UInt64
    let sampleRate: Double
    var samples: [Float] = []
    var windows: [SpeakerEmbeddingWindow] = []
    var nextWindowStartSample = 0
    var failure: SpeakerIdentificationFailure?

    var audioEndNanoseconds: UInt64 {
        let duration = (Double(samples.count) / sampleRate * 1_000_000_000).rounded()
        guard duration.isFinite, duration >= 1 else {
            return audioStartNanoseconds &+ 1
        }
        return audioStartNanoseconds &+ UInt64(min(duration, Double(UInt64.max)))
    }
}

protocol SpeakerEmbeddingPredictor {
    func predict(features: [[Float]]) throws -> [Float]
}

private func sha256Hex(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}

private func validSHA256(_ value: String) -> Bool {
    value.utf8.count == 64
        && value.utf8.allSatisfy { byte in
            (byte >= 48 && byte <= 57)
                || (byte >= 65 && byte <= 70)
                || (byte >= 97 && byte <= 102)
        }
}

final class CoreMLSpeakerEmbeddingPredictor: SpeakerEmbeddingPredictor {
    private let model: MLModel
    private let compiledModelURL: URL
    let modelPackageDigest: String
    private(set) var modelCacheHit = false

    init(path: String, cacheDirectory: URL? = nil) throws {
        guard !path.isEmpty else { throw SpeakerIdentificationFailure.modelPathMissing }
        let url = URL(fileURLWithPath: path, isDirectory: path.hasSuffix(".mlpackage"))
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw SpeakerIdentificationFailure.modelFileMissing(url.path)
        }
        guard url.pathExtension == "mlpackage" else {
            throw SpeakerIdentificationFailure.modelLoad(
                "検証済みの Core ML .mlpackage だけを指定できます"
            )
        }
        let packageHash = try Self.validateModelPackage(at: url)
        let packageURL = url
        let compiledCacheDirectory = cacheDirectory
            ?? url.deletingLastPathComponent().appendingPathComponent("models", isDirectory: true)
        let cacheURL = Self.compiledModelCacheURL(
            packageHash: packageHash,
            cacheDirectory: compiledCacheDirectory
        )
        let preparation = try Self.prepareCompiledModel(
            packageURL: packageURL,
            packageHash: packageHash,
            cacheDirectory: compiledCacheDirectory
        )
        var compiledURL = preparation.url
        var cacheHit = preparation.cacheHit
        let configuration = MLModelConfiguration()
        configuration.computeUnits = .cpuOnly
        let loadedModel: MLModel
        do {
            let candidate = try MLModel(contentsOf: compiledURL, configuration: configuration)
            try Self.validateLoadedModel(candidate)
            loadedModel = candidate
        } catch {
            guard cacheHit else {
                throw SpeakerIdentificationFailure.modelLoad(error.localizedDescription)
            }
            // A cache directory can survive an interrupted copy or an OS update. Remove only
            // this hash-keyed entry and compile it again; other model versions remain intact.
            try? FileManager.default.removeItem(at: cacheURL)
            let rebuilt = try Self.compileAndCache(
                packageURL: packageURL,
                packageHash: packageHash,
                cacheDirectory: compiledCacheDirectory
            )
            do {
                let candidate = try MLModel(contentsOf: rebuilt, configuration: configuration)
                try Self.validateLoadedModel(candidate)
                loadedModel = candidate
                compiledURL = rebuilt
                cacheHit = false
            } catch {
                throw SpeakerIdentificationFailure.modelLoad(error.localizedDescription)
            }
        }
        model = loadedModel
        self.compiledModelURL = compiledURL
        self.modelPackageDigest = packageHash
        self.modelCacheHit = cacheHit
    }

    private struct CachePreparation {
        let url: URL
        let cacheHit: Bool
    }

    private static func validateModelPackage(at url: URL) throws -> String {
        guard url.pathExtension == "mlpackage" else {
            throw SpeakerIdentificationFailure.modelLoad(
                "検証済みの Core ML .mlpackage だけを指定できます"
            )
        }
        for file in speakerIdentificationModelPackageFiles {
            let fileURL = url.appendingPathComponent(file.path)
            guard let data = try? Data(contentsOf: fileURL) else {
                throw SpeakerIdentificationFailure.modelFileMissing(fileURL.path)
            }
            let digest = sha256Hex(data)
            guard digest == file.sha256 else {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML モデルのハッシュが一致しません: \(file.path)"
                )
            }
        }
        let packagePath = url.standardizedFileURL.path
        guard let enumerator = FileManager.default.enumerator(
            at: url,
            includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey]
        ) else {
            throw SpeakerIdentificationFailure.modelLoad("Core ML package を列挙できません")
        }
        var files: [(path: String, data: Data)] = []
        for case let fileURL as URL in enumerator {
            let values: URLResourceValues
            do {
                values = try fileURL.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
            } catch {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML package の属性を読めません: \(error.localizedDescription)"
                )
            }
            guard values.isSymbolicLink != true else {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML package にシンボリックリンクは指定できません"
                )
            }
            guard values.isRegularFile == true else { continue }
            let filePath = fileURL.standardizedFileURL.path
            guard filePath.hasPrefix(packagePath + "/") else {
                throw SpeakerIdentificationFailure.modelLoad("Core ML package のパスが不正です")
            }
            let relativePath = String(filePath.dropFirst(packagePath.count + 1))
            do {
                files.append((relativePath, try Data(contentsOf: fileURL)))
            } catch {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML package のファイルを読めません: \(relativePath)"
                )
            }
        }
        files.sort { $0.path < $1.path }
        guard !files.isEmpty else {
            throw SpeakerIdentificationFailure.modelLoad("Core ML package が空です")
        }
        var packageDigest = SHA256()
        for file in files {
            packageDigest.update(data: Data(file.path.utf8))
            packageDigest.update(data: Data([0]))
            packageDigest.update(data: file.data)
            packageDigest.update(data: Data([0]))
        }
        return packageDigest.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private static func compiledModelCacheURL(packageHash: String, cacheDirectory: URL) -> URL {
        let osVersion = ProcessInfo.processInfo.operatingSystemVersion
        let os = "\(osVersion.majorVersion).\(osVersion.minorVersion).\(osVersion.patchVersion)"
        let coreMLBundle = Bundle(for: MLModel.self)
        let coreMLVersion = (coreMLBundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String)
            ?? (coreMLBundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String)
            ?? "unknown"
        let keyMaterial = [
            speakerIdentificationCompiledModelCacheVersion,
            speakerIdentificationModelIdentifier,
            packageHash,
            os,
            coreMLVersion,
        ].joined(separator: "\n")
        let key = sha256Hex(Data(keyMaterial.utf8))
        return cacheDirectory.appendingPathComponent("\(key).mlmodelc", isDirectory: true)
    }

    private static func prepareCompiledModel(
        packageURL: URL,
        packageHash: String,
        cacheDirectory: URL
    ) throws -> CachePreparation {
        let target = compiledModelCacheURL(
            packageHash: packageHash,
            cacheDirectory: cacheDirectory
        )
        try prepareCacheDirectory(cacheDirectory)
        if FileManager.default.fileExists(atPath: target.path) {
            return CachePreparation(url: target, cacheHit: true)
        }
        let compiled = try compileAndCache(
            packageURL: packageURL,
            packageHash: packageHash,
            cacheDirectory: cacheDirectory
        )
        return CachePreparation(url: compiled, cacheHit: false)
    }

    private static func compileAndCache(
        packageURL: URL,
        packageHash: String,
        cacheDirectory: URL
    ) throws -> URL {
        let target = compiledModelCacheURL(
            packageHash: packageHash,
            cacheDirectory: cacheDirectory
        )
        try prepareCacheDirectory(cacheDirectory)
        if FileManager.default.fileExists(atPath: target.path) {
            return target
        }
        let lockURL = cacheDirectory.appendingPathComponent(
            ".\(target.lastPathComponent).compile.lock"
        )
        guard let lockDescriptor = try acquireCompileLock(at: lockURL, target: target) else {
            return target
        }
        defer {
            close(lockDescriptor)
            unlink(lockURL.path)
        }
        if FileManager.default.fileExists(atPath: target.path) {
            return target
        }
        let compiledURL: URL
        do {
            compiledURL = try MLModel.compileModel(at: packageURL)
        } catch {
            throw SpeakerIdentificationFailure.modelLoad(
                "Core ML モデルをコンパイルできません: " + error.localizedDescription
            )
        }
        let staging = cacheDirectory.appendingPathComponent(
            ".\(target.lastPathComponent).\(UUID().uuidString).tmp",
            isDirectory: true
        )
        do {
            // compileModel の戻り先は一時領域とは限らず別 volume のこともあるため、
            // cache 内へ完全にコピーしてからディレクトリを rename する。
            try FileManager.default.copyItem(at: compiledURL, to: staging)
            try? FileManager.default.removeItem(at: compiledURL)
            do {
                try FileManager.default.moveItem(at: staging, to: target)
            } catch {
                if !FileManager.default.fileExists(atPath: target.path) { throw error }
                try? FileManager.default.removeItem(at: staging)
            }
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o700],
                ofItemAtPath: target.path
            )
            return target
        } catch {
            try? FileManager.default.removeItem(at: staging)
            throw SpeakerIdentificationFailure.modelLoad(
                "コンパイル済み Core ML モデルを保存できません: " + error.localizedDescription
            )
        }
    }

    private static func acquireCompileLock(at url: URL, target: URL) throws -> Int32? {
        let deadline = Date().addingTimeInterval(180)
        while true {
            let descriptor = open(url.path, O_CREAT | O_EXCL | O_WRONLY, 0o600)
            if descriptor >= 0 {
                let data = Data("\(getpid()) \(Date().timeIntervalSince1970)\n".utf8)
                let written = data.withUnsafeBytes { buffer in
                    Darwin.write(descriptor, buffer.baseAddress, data.count)
                }
                guard written == data.count, fsync(descriptor) == 0 else {
                    close(descriptor)
                    unlink(url.path)
                    throw SpeakerIdentificationFailure.modelLoad(
                        "Core ML コンパイルのロックを同期できません"
                    )
                }
                return descriptor
            }
            guard errno == EEXIST else {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML コンパイルのロックを取得できません: errno=\(errno)"
                )
            }
            if FileManager.default.fileExists(atPath: target.path) { return nil }
            try removeStaleCompileLockIfNeeded(at: url)
            guard Date() < deadline else {
                throw SpeakerIdentificationFailure.modelLoad(
                    "Core ML モデルのコンパイルが別の処理で長時間待機しています"
                )
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
    }

    private static func removeStaleCompileLockIfNeeded(at url: URL) throws {
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        let data = try? Data(contentsOf: url)
        let contents = data.flatMap { String(data: $0, encoding: .utf8) }
        if let contents {
            let parts = contents.split(whereSeparator: { $0.isWhitespace })
            if parts.count == 2,
               let pid = Int32(parts[0]),
               let createdAt = TimeInterval(parts[1]),
               pid > 0,
               createdAt.isFinite {
                if kill(pid, 0) == 0 || errno == EPERM { return }
                guard errno == ESRCH,
                      Date().timeIntervalSince1970 - createdAt > speakerLedgerLockStaleSeconds else {
                    return
                }
                try FileManager.default.removeItem(at: url)
                return
            }
        }
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let modifiedAt = attributes[.modificationDate] as? Date,
              Date().timeIntervalSince(modifiedAt) > speakerLedgerLockStaleSeconds else {
            return
        }
        try FileManager.default.removeItem(at: url)
    }

    private static func prepareCacheDirectory(_ directory: URL) throws {
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o700],
                ofItemAtPath: directory.path
            )
        } catch {
            throw SpeakerIdentificationFailure.modelLoad(
                "Core ML モデルのキャッシュを準備できません: " + error.localizedDescription
            )
        }
    }

    private static func validateLoadedModel(_ model: MLModel) throws {
        guard model.modelDescription.inputDescriptionsByName["feats"] != nil,
              model.modelDescription.outputDescriptionsByName["embs"] != nil,
              let creator = model.modelDescription.metadata[MLModelMetadataKey.creatorDefinedKey] as? [String: String],
              creator["com.coosenpai.model.source"]?.contains("Wespeaker/wespeaker-voxceleb-resnet34-LM") == true,
              creator["com.coosenpai.model.embeddingDimension"] == "256" else {
            throw SpeakerIdentificationFailure.modelLoad(
                "指定された Core ML モデルは CooSenpAI 用 WeSpeaker ResNet34-LM ではありません"
            )
        }
    }

    func predict(features: [[Float]]) throws -> [Float] {
        try normalizedEmbedding(predictRaw(features: features))
    }

    func predictRaw(features: [[Float]]) throws -> [Float] {
        guard features.count == speakerWindowFrameCount,
              features.allSatisfy({ $0.count == speakerMelBinCount }) else {
            throw SpeakerIdentificationFailure.modelInputUnavailable
        }
        let input = try MLMultiArray(
            shape: [1, speakerWindowFrameCount, speakerMelBinCount] as [NSNumber],
            dataType: .float32
        )
        let strides = input.strides.map { $0.intValue }
        guard strides.count == 3 else { throw SpeakerIdentificationFailure.modelInputUnavailable }
        let destination = input.dataPointer.bindMemory(
            to: Float.self,
            capacity: input.count
        )
        for frame in 0..<speakerWindowFrameCount {
            for mel in 0..<speakerMelBinCount {
                let index = frame * strides[1] + mel * strides[2]
                destination[index] = features[frame][mel]
            }
        }
        let provider = try MLDictionaryFeatureProvider(dictionary: [
            "feats": MLFeatureValue(multiArray: input)
        ])
        let output: MLFeatureProvider
        do {
            output = try model.prediction(from: provider)
        } catch {
            throw SpeakerIdentificationFailure.modelLoad(error.localizedDescription)
        }
        guard let value = output.featureValue(for: "embs")?.multiArrayValue,
              value.count == 256 else {
            throw SpeakerIdentificationFailure.modelOutputInvalid
        }
        let source = value.dataPointer.bindMemory(to: Float.self, capacity: value.count)
        var embedding = Array(repeating: Float.zero, count: value.count)
        let outputStrides = value.strides.map { $0.intValue }
        if outputStrides.count == 2 {
            for index in embedding.indices {
                embedding[index] = source[index * outputStrides[1]]
            }
        } else {
            for index in embedding.indices { embedding[index] = source[index] }
        }
        return embedding
    }
}

func speakerModelCacheDirectory(for ledgerPath: String) -> URL {
    URL(fileURLWithPath: ledgerPath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("models", isDirectory: true)
}

enum WeSpeakerFbank {
    static func features(for samples: [Float]) throws -> [[Float]] {
        guard samples.count == speakerWindowSampleCount else {
            throw SpeakerIdentificationFailure.modelInputUnavailable
        }
        let scaled = samples.map { $0 * 32_768.0 }
        var frames = Array(
            repeating: Array(repeating: Float.zero, count: speakerMelBinCount),
            count: speakerWindowFrameCount
        )
        let melFilters = makeMelFilters()
        for frameIndex in 0..<speakerWindowFrameCount {
            let start = frameIndex * speakerFrameShiftSampleCount
            var frameInput = Array(repeating: Float.zero, count: speakerFrameSampleCount)
            for index in frameInput.indices {
                frameInput[index] = scaled[start + index]
            }
            let frameMean = frameInput.reduce(Float.zero, +) / Float(speakerFrameSampleCount)
            var window = Array(repeating: Float.zero, count: 512)
            for index in 0..<speakerFrameSampleCount {
                let value = frameInput[index] - frameMean
                let preemphasized: Float
                if index == 0 {
                    preemphasized = value * (1.0 - 0.97)
                } else {
                    preemphasized = value - 0.97 * (frameInput[index - 1] - frameMean)
                }
                let hamming = 0.54 - 0.46 * cos(
                    (2.0 * Double.pi * Double(index)) / Double(speakerFrameSampleCount - 1)
                )
                window[index] = preemphasized * Float(hamming)
            }
            let spectrum = try powerSpectrum(window)
            for melIndex in 0..<speakerMelBinCount {
                var energy: Float = 0
                for bin in melFilters[melIndex].indices {
                    energy += spectrum[bin] * melFilters[melIndex][bin]
                }
                frames[frameIndex][melIndex] = log(max(energy, Float.ulpOfOne))
            }
        }
        for melIndex in 0..<speakerMelBinCount {
            let mean = frames.reduce(Float.zero) { $0 + $1[melIndex] }
                / Float(speakerWindowFrameCount)
            for frameIndex in frames.indices {
                frames[frameIndex][melIndex] -= mean
            }
        }
        return frames
    }

    private static func makeMelFilters() -> [[Float]] {
        let fftBinCount = 256
        let lowMel = 1127.0 * log(1.0 + 20.0 / 700.0)
        let highMel = 1127.0 * log(1.0 + 8_000.0 / 700.0)
        let melDelta = (highMel - lowMel) / Double(speakerMelBinCount + 1)
        let leftMels = (0..<speakerMelBinCount).map { lowMel + Double($0) * melDelta }
        let centerMels = (0..<speakerMelBinCount).map { lowMel + Double($0 + 1) * melDelta }
        let rightMels = (0..<speakerMelBinCount).map { lowMel + Double($0 + 2) * melDelta }
        return (0..<speakerMelBinCount).map { melIndex in
            var filter = Array(repeating: Float.zero, count: fftBinCount)
            let left = leftMels[melIndex]
            let center = centerMels[melIndex]
            let right = rightMels[melIndex]
            for bin in filter.indices {
                let frequency = speakerSampleRate * Double(bin) / 512.0
                let mel = 1127.0 * log(1.0 + frequency / 700.0)
                let upSlope = (mel - left) / (center - left)
                let downSlope = (right - mel) / (right - center)
                filter[bin] = Float(max(0.0, min(upSlope, downSlope)))
            }
            return filter
        }
    }

    private static func powerSpectrum(_ input: [Float]) throws -> [Float] {
        precondition(input.count == 512)
        guard let setup = vDSP_create_fftsetup(9, FFTRadix(kFFTRadix2)) else {
            throw SpeakerIdentificationFailure.modelInputUnavailable
        }
        defer { vDSP_destroy_fftsetup(setup) }

        var splitReal = Array(repeating: Float.zero, count: 256)
        var splitImaginary = Array(repeating: Float.zero, count: 256)
        var spectrum = Array(repeating: Float.zero, count: 257)
        splitReal.withUnsafeMutableBufferPointer { realBuffer in
            splitImaginary.withUnsafeMutableBufferPointer { imaginaryBuffer in
                var splitComplex = DSPSplitComplex(
                    realp: realBuffer.baseAddress!,
                    imagp: imaginaryBuffer.baseAddress!
                )
                input.withUnsafeBufferPointer { inputBuffer in
                    inputBuffer.baseAddress!.withMemoryRebound(to: DSPComplex.self, capacity: 256) { complexInput in
                        vDSP_ctoz(complexInput, 2, &splitComplex, 1, 256)
                    }
                    vDSP_fft_zrip(setup, &splitComplex, 1, 9, FFTDirection(FFT_FORWARD))
                }
            }
        }
        // vDSP の実数 FFT は通常の複素 FFT の振幅を 2 倍で返す。
        // torchaudio の rfft と同じ power にそろえるため 1/4 を掛ける。
        spectrum[0] = splitReal[0] * splitReal[0] * 0.25
        for index in 1..<256 {
            spectrum[index] = splitReal[index] * splitReal[index]
                + splitImaginary[index] * splitImaginary[index]
            spectrum[index] *= 0.25
        }
        spectrum[256] = splitImaginary[0] * splitImaginary[0] * 0.25
        return spectrum
    }
}

private func normalizedEmbedding(_ value: [Float]) throws -> [Float] {
    guard value.count == 256,
          value.allSatisfy({ $0.isFinite }) else {
        throw SpeakerIdentificationFailure.invalidEmbedding
    }
    let norm = sqrt(value.reduce(Float.zero) { $0 + $1 * $1 })
    guard norm.isFinite, norm > 0 else { throw SpeakerIdentificationFailure.invalidEmbedding }
    return value.map { $0 / norm }
}

private func cosineSimilarity(_ left: [Float], _ right: [Float]) -> Float {
    guard left.count == right.count else { return -1 }
    return left.enumerated().reduce(Float.zero) { result, item in
        result + item.element * right[item.offset]
    }
}

private func speakerNumber(_ value: String) -> UInt64? {
    let prefix = "speaker-"
    guard value.hasPrefix(prefix) else {
        return nil
    }
    let number = String(value.dropFirst(prefix.count))
    guard !number.isEmpty else { return nil }
    return UInt64(number).flatMap { $0 > 0 ? $0 : nil }
}

private func validSpeakerID(_ value: String) -> Bool {
    speakerNumber(value) != nil
}

private struct SpeakerProfile: Codable {
    let id: String
    let anchor: [Float]
    var centroid: [Float]
    var updateCount: UInt32
    var state: String
}

private struct PendingSpeakerCandidate {
    let generation: Int
    let embedding: [Float]
    let weight: Float
    let observedAt: Date
}

private struct SpeakerRegistryDocument: Codable {
    let schemaVersion: Int
    let registryID: String
    var nextSpeakerNumber: UInt64
    let modelIdentifier: String
    let modelPackageDigest: String
    let preprocessingVersion: String
    let decisionVersion: String
    var profiles: [SpeakerProfile]
    var aliases: [String: String]
    var revision: UInt64
}

private struct SpeakerRegistryEnvelope: Codable {
    let schemaVersion: Int
    let combined: String
}

struct SpeakerAliasIndex: Codable {
    let schemaVersion: Int
    let registryID: String
    let aliases: [String: String]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case registryID = "registryId"
        case aliases
    }

    private enum LegacyCodingKeys: String, CodingKey {
        case registryID
    }
}

extension SpeakerAliasIndex {
    // v0.4.0 は registryID キーで書き出していたため、既存の別名索引も読めるようにする。
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(Int.self, forKey: .schemaVersion)
        if let registryID = try container.decodeIfPresent(String.self, forKey: .registryID) {
            self.registryID = registryID
        } else {
            registryID = try decoder
                .container(keyedBy: LegacyCodingKeys.self)
                .decode(String.self, forKey: .registryID)
        }
        aliases = try container.decode([String: String].self, forKey: .aliases)
    }
}

protocol SpeakerKeyStore: AnyObject {
    func readExisting() throws -> SymmetricKey?
    func readOrCreate() throws -> SymmetricKey
    func delete() throws
}

private struct SpeakerGenerationStart {
    let previousGeneration: Int?
}

private final class SpeakerCommitStateMachine {
    private enum ReservationState {
        case reserved
        case persisting
        case cancelled
    }

    private struct Reservation {
        let generation: Int
        let deadline: DispatchTime
        var state: ReservationState
    }

    private let lock = NSLock()
    private var currentGeneration: Int?
    private var highestGeneration: Int?
    private var invalidatedGenerations = Set<Int>()
    private var shutdownRequested = false
    private var reservations: [UUID: Reservation] = [:]

    func startGeneration(_ generation: Int) -> SpeakerGenerationStart? {
        lock.lock()
        defer { lock.unlock() }
        guard !shutdownRequested,
              highestGeneration.map({ generation > $0 }) ?? true,
              !invalidatedGenerations.contains(generation) else {
            return nil
        }
        let previousGeneration = currentGeneration
        if let previousGeneration {
            invalidatedGenerations.insert(previousGeneration)
        }
        highestGeneration = generation
        currentGeneration = generation
        pruneInvalidatedGenerationsLocked()
        for token in Array(reservations.keys) {
            guard reservations[token]?.generation != generation,
                  reservations[token]?.state == .reserved else { continue }
            reservations[token]?.state = .cancelled
        }
        return SpeakerGenerationStart(previousGeneration: previousGeneration)
    }

    func cancelGeneration(_ generation: Int) {
        lock.lock()
        defer { lock.unlock() }
        invalidateGenerationLocked(generation)
    }

    func cancelCurrentGeneration() -> Int? {
        lock.lock()
        defer { lock.unlock() }
        guard let generation = currentGeneration else { return nil }
        invalidateGenerationLocked(generation)
        return generation
    }

    func finishGeneration(_ generation: Int) {
        lock.lock()
        defer { lock.unlock() }
        invalidatedGenerations.insert(generation)
        pruneInvalidatedGenerationsLocked()
        if currentGeneration == generation { currentGeneration = nil }
    }

    func shutdown() {
        lock.lock()
        defer { lock.unlock() }
        shutdownRequested = true
        currentGeneration = nil
        for token in Array(reservations.keys) {
            guard reservations[token]?.state == .reserved else { continue }
            reservations[token]?.state = .cancelled
        }
    }

    func isGenerationActive(_ generation: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return !shutdownRequested
            && currentGeneration == generation
            && !invalidatedGenerations.contains(generation)
    }

    func reserve(
        generation: Int,
        deadline: DispatchTime
    ) -> SpeakerLedgerCommitPermit? {
        lock.lock()
        defer { lock.unlock() }
        guard !shutdownRequested,
              currentGeneration == generation,
              !invalidatedGenerations.contains(generation),
              hasCommitTime(until: deadline) else {
            return nil
        }
        let token = UUID()
        reservations[token] = Reservation(
            generation: generation,
            deadline: deadline,
            state: .reserved
        )
        return SpeakerLedgerCommitPermit(stateMachine: self, token: token)
    }

    func reservation(for generation: Int) -> SpeakerLedgerCommitPermit? {
        lock.lock()
        defer { lock.unlock() }
        guard let token = reservations.first(where: {
            $0.value.generation == generation
                && ($0.value.state == .reserved || $0.value.state == .persisting)
        })?.key else {
            return nil
        }
        return SpeakerLedgerCommitPermit(stateMachine: self, token: token)
    }

    fileprivate func cancel(token: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard reservations[token]?.state == .reserved else { return false }
        reservations[token]?.state = .cancelled
        return true
    }

    fileprivate func beginPersistence(token: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard let reservation = reservations[token],
              reservation.state == .reserved,
              currentGeneration == reservation.generation,
              !shutdownRequested,
              !invalidatedGenerations.contains(reservation.generation),
              hasCommitTime(until: reservation.deadline) else {
            reservations[token]?.state = .cancelled
            return false
        }
        reservations[token]?.state = .persisting
        return true
    }

    fileprivate func isActive(token: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard let state = reservations[token]?.state else { return false }
        return state == .reserved || state == .persisting
    }

    fileprivate func isPersisting(token: UUID) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return reservations[token]?.state == .persisting
    }

    fileprivate func withActiveReservation<T>(token: UUID, _ body: () throws -> T) throws -> T {
        lock.lock()
        defer { lock.unlock() }
        guard let reservation = reservations[token],
              reservation.state == .reserved,
              !shutdownRequested,
              currentGeneration == reservation.generation,
              !invalidatedGenerations.contains(reservation.generation) else {
            throw SpeakerIdentificationFailure.deadlineExceeded
        }
        return try body()
    }

    fileprivate func release(token: UUID) {
        lock.lock()
        reservations.removeValue(forKey: token)
        lock.unlock()
    }

    private func hasCommitTime(until deadline: DispatchTime) -> Bool {
        let now = DispatchTime.now()
        return now < deadline
            && deadline.uptimeNanoseconds - now.uptimeNanoseconds
                > speakerLedgerCommitSafetyNanoseconds
    }

    private func invalidateGenerationLocked(_ generation: Int) {
        invalidatedGenerations.insert(generation)
        pruneInvalidatedGenerationsLocked()
        if currentGeneration == generation { currentGeneration = nil }
        for token in Array(reservations.keys) {
            guard reservations[token]?.generation == generation,
                  reservations[token]?.state == .reserved else { continue }
            reservations[token]?.state = .cancelled
        }
    }

    private func pruneInvalidatedGenerationsLocked() {
        if let highestGeneration {
            let lowerBound = highestGeneration > speakerMaximumInvalidatedGenerations
                ? highestGeneration - speakerMaximumInvalidatedGenerations
                : Int.min
            invalidatedGenerations = Set(invalidatedGenerations.filter { $0 >= lowerBound })
        }
        while invalidatedGenerations.count > speakerMaximumInvalidatedGenerations {
            guard let oldest = invalidatedGenerations.min() else { break }
            invalidatedGenerations.remove(oldest)
        }
    }
}

final class SpeakerLedgerCommitPermit {
    private weak var stateMachine: SpeakerCommitStateMachine?
    private let token: UUID

    fileprivate init(stateMachine: SpeakerCommitStateMachine, token: UUID) {
        self.stateMachine = stateMachine
        self.token = token
    }

    func cancel() -> Bool {
        stateMachine?.cancel(token: token) ?? false
    }

    func beginPersistence() -> Bool {
        stateMachine?.beginPersistence(token: token) ?? false
    }

    func isActive() -> Bool {
        stateMachine?.isActive(token: token) ?? false
    }

    func isPersisting() -> Bool {
        stateMachine?.isPersisting(token: token) ?? false
    }

    fileprivate func withActiveReservation<T>(_ body: () throws -> T) throws -> T {
        guard let stateMachine else {
            throw SpeakerIdentificationFailure.deadlineExceeded
        }
        return try stateMachine.withActiveReservation(token: token, body)
    }

    func release() {
        stateMachine?.release(token: token)
    }

    deinit { release() }
}

typealias SpeakerLedgerCommitAuthorizer = () -> SpeakerLedgerCommitPermit?

private final class SpeakerDiagnosisKeyStore: SpeakerKeyStore {
    private var key: SymmetricKey?

    func readExisting() throws -> SymmetricKey? { key }

    func readOrCreate() throws -> SymmetricKey {
        if let key { return key }
        let key = SymmetricKey(size: .bits256)
        self.key = key
        return key
    }

    func delete() throws { key = nil }
}

private final class SpeakerKeychain: SpeakerKeyStore {
    private let account: String
    private let service = "dev.nrslib.coosenpai.speaker-registry"

    init(path: URL) {
        account = "registry:\(path.standardizedFileURL.path)"
    }

    func readExisting() throws -> SymmetricKey? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else {
            throw SpeakerIdentificationFailure.ledgerKeyUnavailable(status)
        }
        guard let data = result as? Data, data.count == 32 else {
            throw SpeakerIdentificationFailure.ledgerKeyUnavailable(errSecDecode)
        }
        return SymmetricKey(data: data)
    }

    func readOrCreate() throws -> SymmetricKey {
        if let key = try readExisting() { return key }
        let key = SymmetricKey(size: .bits256)
        let data = key.withUnsafeBytes { Data($0) }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
            kSecValueData as String: data,
        ]
        let addStatus = SecItemAdd(query as CFDictionary, nil)
        guard addStatus == errSecSuccess || addStatus == errSecDuplicateItem else {
            throw SpeakerIdentificationFailure.ledgerKeyUnavailable(addStatus)
        }
        if addStatus == errSecDuplicateItem {
            return try readOrCreate()
        }
        return key
    }

    func delete() throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecAttrSynchronizable as String: kCFBooleanFalse as Any,
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw SpeakerIdentificationFailure.ledgerKeyUnavailable(status)
        }
    }
}

final class SpeakerLedger {
    private let path: URL
    private let keyStore: SpeakerKeyStore
    private var expectedModelPackageDigest: String?
    private let beforePersistence: () -> Void
    private let afterPersistenceBegan: () -> Void
    private let afterCommitReserved: () -> Void
    private let shouldFailAliasWrite: () -> Bool
    private var document: SpeakerRegistryDocument?
    private var pendingCandidates: [PendingSpeakerCandidate] = []
    private var identificationFailure: SpeakerIdentificationFailure?

    init(
        path: String,
        keyStore: SpeakerKeyStore? = nil,
        modelPackageDigest: String? = nil,
        beforePersistence: @escaping () -> Void = {},
        afterPersistenceBegan: @escaping () -> Void = {},
        afterCommitReserved: @escaping () -> Void = {},
        shouldFailAliasWrite: @escaping () -> Bool = { false }
    ) throws {
        guard !path.isEmpty else { throw SpeakerIdentificationFailure.ledgerPathMissing }
        self.path = URL(fileURLWithPath: path)
        self.keyStore = keyStore ?? SpeakerKeychain(path: self.path)
        if let modelPackageDigest, !validSHA256(modelPackageDigest) {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        self.expectedModelPackageDigest = modelPackageDigest
        self.beforePersistence = beforePersistence
        self.afterPersistenceBegan = afterPersistenceBegan
        self.afterCommitReserved = afterCommitReserved
        self.shouldFailAliasWrite = shouldFailAliasWrite
        if Self.fileExists(at: self.lockPath) {
            try Self.removeStaleLockIfNeeded(at: self.lockPath)
            guard !Self.fileExists(at: self.lockPath) else {
                throw SpeakerIdentificationFailure.ledgerLocked
            }
        }
        if Self.fileExists(at: self.path) {
            try load()
        } else if try self.keyStore.readExisting() != nil {
            throw SpeakerIdentificationFailure.ledgerMissing
        }
        if let modelPackageDigest,
           let document,
           document.modelPackageDigest != modelPackageDigest {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
    }

    convenience init(
        forDiagnosisAt path: String,
        modelPackageDigest: String
    ) throws {
        try self.init(
            path: path,
            keyStore: SpeakerDiagnosisKeyStore(),
            modelPackageDigest: modelPackageDigest
        )
    }

    var activeProfileCount: Int {
        document?.profiles.filter { $0.state == "active" }.count ?? 0
    }

    private var lockPath: URL {
        path.appendingPathExtension("lock")
    }

    private var aliasesPath: URL {
        path.deletingLastPathComponent().appendingPathComponent("aliases.json")
    }

    private static func fileExists(at url: URL) -> Bool {
        FileManager.default.fileExists(atPath: url.path)
    }

    private func removeAliasIndex() throws {
        guard Self.fileExists(at: aliasesPath) else { return }
        do {
            try FileManager.default.removeItem(at: aliasesPath)
        } catch {
            throw SpeakerIdentificationFailure.ledgerWrite(
                "別名索引を削除できません: \(error.localizedDescription)"
            )
        }
    }

    func bindModelPackageDigest(_ digest: String) throws {
        guard validSHA256(digest) else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        if let expectedModelPackageDigest, expectedModelPackageDigest != digest {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        if let document, document.modelPackageDigest != digest {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        expectedModelPackageDigest = digest
    }

    private static func removeStaleLockIfNeeded(at url: URL) throws {
        guard fileExists(at: url) else { return }
        let data = try? Data(contentsOf: url)
        let contents = data.flatMap { String(data: $0, encoding: .utf8) }
        if let contents {
            let parts = contents.split(whereSeparator: { $0.isWhitespace })
            if parts.count == 2,
               let pid = Int32(parts[0]),
               let createdAt = TimeInterval(parts[1]),
               pid > 0,
               createdAt.isFinite {
                if kill(pid, 0) == 0 || errno == EPERM { return }
                guard errno == ESRCH,
                      Date().timeIntervalSince1970 - createdAt > speakerLedgerLockStaleSeconds else {
                    return
                }
                try FileManager.default.removeItem(at: url)
                return
            }
        }

        // メタデータを書き込む前に異常終了したロックも、十分古ければ回収する。
        // 保存処理はこの時間を超えてロックを保持しない契約なので、現行処理を奪わない。
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: url.path),
              let modifiedAt = attributes[.modificationDate] as? Date,
              Date().timeIntervalSince(modifiedAt) > speakerLedgerLockStaleSeconds else {
            return
        }
        try FileManager.default.removeItem(at: url)
    }

    private struct SpeakerDecisionAnalysis {
        let allEvidenceWindows: [SpeakerEmbeddingWindow]
        let evidenceWindows: [SpeakerEmbeddingWindow]
        let segmentEmbedding: [Float]
        let rankedCandidates: [(id: String, score: Float, anchor: Float)]
        let pairwiseSimilarities: [Float]
        let primaryClusterCount: Int
        let secondaryClusterCount: Int
        let mixed: Bool
        let knownMaximum: Float
        let canEnroll: Bool
        let windowConsistencyThreshold: Float
        let knownSimilarityThreshold: Float
        let singleWindowSimilarityThreshold: Float
        let newSpeakerSimilarityThreshold: Float
        let allowsSingleWindowEnrollment: Bool
        let allowsCentroidUpdate: Bool
    }

    private func analyze(
        windows: [SpeakerEmbeddingWindow],
        rule: SpeakerDecisionRule
    ) throws -> SpeakerDecisionAnalysis? {
        guard !windows.isEmpty else { return nil }
        guard windows.allSatisfy({ $0.embedding.count == 256 }) else {
            throw SpeakerIdentificationFailure.invalidEmbedding
        }
        let activeProfiles = document?.profiles.filter { $0.state == "active" } ?? []
        let allEvidenceWindows = nonOverlappingWindows(windows)
        guard !allEvidenceWindows.isEmpty else { return nil }
        let pairwiseSimilarities = pairwiseCosineSimilarities(allEvidenceWindows)
        let windowConsistencyThreshold = rule.windowConsistencyThreshold
        let primaryClusterCount: Int
        let secondaryClusterCount: Int
        let mixed: Bool
        switch rule.windowClustering {
        case .allPairwise:
            primaryClusterCount = allEvidenceWindows.count
            secondaryClusterCount = 0
            mixed = pairwiseSimilarities.contains { $0 < windowConsistencyThreshold }
        case .dominantCluster:
            let clusterSelection = speakerWindowClusterSelection(
                allEvidenceWindows,
                consistencyThreshold: windowConsistencyThreshold
            )
            primaryClusterCount = clusterSelection.primary.count
            secondaryClusterCount = clusterSelection.secondary.count
            mixed = clusterSelection.isMixed
        }
        let segmentEmbedding = try normalizedEmbedding(weightedAverage(allEvidenceWindows))
        let candidateScores = activeProfiles.compactMap { profile -> (String, Float, Float)? in
            let anchor = cosineSimilarity(segmentEmbedding, profile.anchor)
            return (canonicalID(profile.id), anchor, anchor)
        }
        let rankedCandidates = rankCandidates(candidateScores)
        let knownMaximum = activeProfiles
            .map { profile in cosineSimilarity(segmentEmbedding, profile.anchor) }
            .max() ?? -1
        let canEnroll = allEvidenceWindows.count >= 2
            && !mixed
            && knownMaximum <= rule.newSpeakerSimilarityThreshold
        let allowsCentroidUpdate = speakerCentroidAutoUpdateEnabled
            && allEvidenceWindows.count >= speakerCentroidUpdateMinimumWindowCount
            && rankedCandidates.count >= 2
            && rankedCandidates[0].score >= speakerCentroidUpdateSimilarityThreshold
            && rankedCandidates[0].score - rankedCandidates[1].score
                >= speakerCentroidUpdateMarginThreshold
        return SpeakerDecisionAnalysis(
            allEvidenceWindows: allEvidenceWindows,
            evidenceWindows: allEvidenceWindows,
            segmentEmbedding: segmentEmbedding,
            rankedCandidates: rankedCandidates,
            pairwiseSimilarities: pairwiseSimilarities,
            primaryClusterCount: primaryClusterCount,
            secondaryClusterCount: secondaryClusterCount,
            mixed: mixed,
            knownMaximum: knownMaximum,
            canEnroll: canEnroll,
            windowConsistencyThreshold: windowConsistencyThreshold,
            knownSimilarityThreshold: rule.knownSimilarityThreshold,
            singleWindowSimilarityThreshold: rule.singleWindowSimilarityThreshold,
            newSpeakerSimilarityThreshold: rule.newSpeakerSimilarityThreshold,
            allowsSingleWindowEnrollment: rule.allowsSingleWindowEnrollment,
            allowsCentroidUpdate: allowsCentroidUpdate
        )
    }

    private func status(for analysis: SpeakerDecisionAnalysis)
        -> SpeakerIdentificationStatusValue {
        if analysis.mixed {
            return .mixed
        }
        let threshold = analysis.evidenceWindows.count == 1
            ? analysis.singleWindowSimilarityThreshold
            : analysis.knownSimilarityThreshold
        if let first = analysis.rankedCandidates.first,
           first.score >= threshold,
           analysis.rankedCandidates.count == 1
               || first.score - analysis.rankedCandidates[1].score >= speakerMarginThreshold {
            return .identified
        }
        return analysis.canEnroll ? .identified : .unknown
    }

    func diagnose(windows: [SpeakerEmbeddingWindow]) throws -> SpeakerIdentificationDiagnostic {
        return try diagnose(windows: windows, rule: .dominantCluster)
    }

    func diagnose(
        windows: [SpeakerEmbeddingWindow],
        rule: SpeakerDecisionRule
    ) throws -> SpeakerIdentificationDiagnostic {
        guard let analysis = try analyze(windows: windows, rule: rule) else {
            return SpeakerIdentificationDiagnostic(
                windowCount: 0,
                pairwiseSimilarities: [],
                candidateScores: [],
                primaryClusterCount: 0,
                secondaryClusterCount: 0,
                status: .unknown
            )
        }
        return SpeakerIdentificationDiagnostic(
            windowCount: analysis.allEvidenceWindows.count,
            pairwiseSimilarities: analysis.pairwiseSimilarities,
            candidateScores: analysis.rankedCandidates.map {
                (id: $0.id, score: $0.score)
            },
            primaryClusterCount: analysis.primaryClusterCount,
            secondaryClusterCount: analysis.secondaryClusterCount,
            status: status(for: analysis)
        )
    }

    func identify(
        windows: [SpeakerEmbeddingWindow],
        generation: Int = 0,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil
    ) throws -> (String?, String?, SpeakerIdentificationStatusValue) {
        if let identificationFailure {
            throw identificationFailure
        }
        guard let analysis = try analyze(windows: windows, rule: .dominantCluster) else {
            return (nil, nil, .unknown)
        }
        return try identify(
            analysis: analysis,
            generation: generation,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
    }

    func identifyForDiagnosis(
        windows: [SpeakerEmbeddingWindow],
        generation: Int,
        rule: SpeakerDecisionRule
    ) throws -> SpeakerDiagnosisIdentification {
        if let identificationFailure {
            throw identificationFailure
        }
        guard let analysis = try analyze(windows: windows, rule: rule) else {
            return SpeakerDiagnosisIdentification(
                diagnostic: SpeakerIdentificationDiagnostic(
                    windowCount: 0,
                    pairwiseSimilarities: [],
                    candidateScores: [],
                    primaryClusterCount: 0,
                    secondaryClusterCount: 0,
                    status: .unknown
                ),
                speakerID: nil,
                registryID: nil,
                segmentEmbedding: nil
            )
        }
        let result = try identify(analysis: analysis, generation: generation)
        return SpeakerDiagnosisIdentification(
            diagnostic: SpeakerIdentificationDiagnostic(
                windowCount: analysis.allEvidenceWindows.count,
                pairwiseSimilarities: analysis.pairwiseSimilarities,
                candidateScores: analysis.rankedCandidates.map {
                    (id: $0.id, score: $0.score)
                },
                primaryClusterCount: analysis.primaryClusterCount,
                secondaryClusterCount: analysis.secondaryClusterCount,
                status: result.2
            ),
            speakerID: result.0,
            registryID: result.1,
            segmentEmbedding: analysis.segmentEmbedding
        )
    }

    private func identify(
        analysis: SpeakerDecisionAnalysis,
        generation: Int,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil
    ) throws -> (String?, String?, SpeakerIdentificationStatusValue) {
        if analysis.mixed {
            return (nil, nil, .mixed)
        }
        let threshold = analysis.evidenceWindows.count == 1
            ? analysis.singleWindowSimilarityThreshold
            : analysis.knownSimilarityThreshold
        if let first = analysis.rankedCandidates.first,
           first.score >= threshold,
           (analysis.rankedCandidates.count == 1
               || first.score - analysis.rankedCandidates[1].score >= speakerMarginThreshold) {
            if analysis.allowsCentroidUpdate {
                try updateCentroid(
                    id: first.id,
                    embedding: analysis.segmentEmbedding,
                    canCommit: canCommit,
                    authorizeCommit: authorizeCommit
                )
            }
            return (first.id, document?.registryID, .identified)
        }
        if analysis.canEnroll {
            let id = try register(
                embedding: analysis.segmentEmbedding,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            return (id, document?.registryID, .identified)
        }
        guard analysis.evidenceWindows.count > 1 || analysis.allowsSingleWindowEnrollment else {
            return (nil, nil, .unknown)
        }
        guard analysis.knownMaximum <= analysis.newSpeakerSimilarityThreshold else {
            return (nil, nil, .unknown)
        }
        return try enrollOrRememberCandidate(
            from: [
                SpeakerEmbeddingWindow(
                    embedding: analysis.segmentEmbedding,
                    startSample: 0,
                    weight: 1
                ),
            ],
            generation: generation,
            consistencyThreshold: analysis.windowConsistencyThreshold,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
    }

    func discardPendingCandidates(for generation: Int) {
        pendingCandidates.removeAll { $0.generation == generation }
    }

    func merge(from source: String, to target: String) throws {
        guard let currentDocument = document,
              source != target,
              currentDocument.aliases[source] == nil,
              currentDocument.profiles.contains(where: { $0.id == source && $0.state == "active" }),
              validSpeakerID(target) else {
            throw SpeakerIdentificationFailure.ledgerWrite("統合対象の話者 ID が見つかりません")
        }
        let canonicalTarget = canonicalID(target)
        guard canonicalTarget != source,
              currentDocument.profiles.contains(where: { $0.id == canonicalTarget && $0.state == "active" }) else {
            throw SpeakerIdentificationFailure.ledgerWrite("統合対象の話者 ID が見つかりません")
        }
        var document = try writableDocument()
        document.aliases[source] = canonicalTarget
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    func undoMerge(source: String) throws {
        var document = try writableDocument()
        guard document.aliases.removeValue(forKey: source) != nil else {
            throw SpeakerIdentificationFailure.ledgerWrite("取り消す統合が見つかりません")
        }
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    func reregister(id: String) throws {
        var document = try writableDocument()
        guard let index = document.profiles.firstIndex(where: { $0.id == id }) else {
            throw SpeakerIdentificationFailure.ledgerWrite("再登録対象の話者 ID が見つかりません")
        }
        document.profiles[index].state = "retired"
        document.aliases = document.aliases.filter { $0.key != id && $0.value != id }
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    func delete(id: String) throws {
        var document = try writableDocument()
        guard document.profiles.contains(where: { $0.id == id }) else {
            throw SpeakerIdentificationFailure.ledgerWrite("削除対象の話者 ID が見つかりません")
        }
        document.profiles.removeAll { $0.id == id }
        document.aliases = document.aliases.filter { $0.key != id && $0.value != id }
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    func deleteAll() throws {
        guard var document else {
            pendingCandidates.removeAll()
            try removeAliasIndex()
            return
        }
        document.profiles.removeAll()
        document.aliases.removeAll()
        document.revision &+= 1
        try save(document)
        self.document = document
        pendingCandidates.removeAll()
    }

    private func load() throws {
        do {
            let envelopeData = try Data(contentsOf: path)
            let envelope = try JSONDecoder().decode(SpeakerRegistryEnvelope.self, from: envelopeData)
            guard envelope.schemaVersion == 1,
                  let combined = Data(base64Encoded: envelope.combined) else {
                throw SpeakerIdentificationFailure.ledgerCorrupt
            }
            guard let key = try keyStore.readExisting() else {
                throw SpeakerIdentificationFailure.ledgerKeyUnavailable(errSecItemNotFound)
            }
            let box = try AES.GCM.SealedBox(combined: combined)
            let data = try AES.GCM.open(box, using: key)
            let decoded = try JSONDecoder().decode(SpeakerRegistryDocument.self, from: data)
            try validate(decoded)
            guard decoded.modelIdentifier == speakerIdentificationModelIdentifier,
                  decoded.preprocessingVersion == speakerIdentificationPreprocessingVersion else {
                throw SpeakerIdentificationFailure.ledgerCorrupt
            }
            guard decoded.decisionVersion == speakerIdentificationDecisionVersion else {
                throw SpeakerIdentificationFailure.decisionVersionMismatch
            }
            document = decoded
            try rebuildAliasIndex(from: decoded)
        } catch let error as SpeakerIdentificationFailure {
            throw error
        } catch {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
    }

    private func rebuildAliasIndex(from document: SpeakerRegistryDocument) throws {
        let index = SpeakerAliasIndex(
            schemaVersion: 1,
            registryID: document.registryID,
            aliases: document.aliases
        )
        do {
            let data = try JSONEncoder().encode(index)
            guard data.count <= speakerMaximumRegistryBytes else {
                throw SpeakerIdentificationFailure.ledgerAliasWrite(
                    "別名索引の上限を超えました"
                )
            }
            if shouldFailAliasWrite() {
                throw SpeakerIdentificationFailure.ledgerAliasWrite(
                    "テスト用に別名索引の保存を失敗させました"
                )
            }
            try Self.atomicWrite(data, to: aliasesPath)
        } catch {
            throw aliasWriteFailure(error)
        }
    }

    private func aliasWriteFailure(_ error: Error) -> SpeakerIdentificationFailure {
        if let failure = error as? SpeakerIdentificationFailure,
           case .ledgerAliasWrite = failure {
            return failure
        }
        return .ledgerAliasWrite(error.localizedDescription)
    }

    private func validate(_ document: SpeakerRegistryDocument) throws {
        var profileIDs = Set<String>()
        guard document.schemaVersion == 1,
              UUID(uuidString: document.registryID) != nil,
              document.nextSpeakerNumber > 0,
              validSHA256(document.modelPackageDigest),
              document.profiles.count <= speakerMaximumProfiles,
              document.profiles.allSatisfy({
                  validSpeakerID($0.id)
                      && profileIDs.insert($0.id).inserted
                      && ($0.state == "active" || $0.state == "retired")
                      && (speakerNumber($0.id) ?? 0) < document.nextSpeakerNumber
                      && $0.anchor.count == 256 && $0.centroid.count == 256
                      && $0.anchor.allSatisfy { $0.isFinite }
                      && $0.centroid.allSatisfy { $0.isFinite }
              })
              && document.aliases.allSatisfy({ source, target in
                  validSpeakerID(source)
                      && validSpeakerID(target)
                      && source != target
                      && profileIDs.contains(source)
                      && profileIDs.contains(target)
              }) else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        for source in document.aliases.keys {
            var current = source
            var visited = Set<String>()
            while let next = document.aliases[current] {
                guard visited.insert(current).inserted, profileIDs.contains(next) else {
                    throw SpeakerIdentificationFailure.ledgerCorrupt
                }
                current = next
            }
        }
    }

    private func writableDocument() throws -> SpeakerRegistryDocument {
        guard let document else { throw SpeakerIdentificationFailure.ledgerWrite("話者台帳がありません") }
        return document
    }

    private func profile(for id: String) -> SpeakerProfile? {
        let canonical = canonicalID(id)
        return document?.profiles.first { $0.id == canonical && $0.state == "active" }
    }

    private func canonicalID(_ id: String) -> String {
        var current = id
        var visited = Set<String>()
        while let next = document?.aliases[current], visited.insert(current).inserted {
            current = next
        }
        return current
    }

    private func rankCandidates(_ candidates: [(id: String, score: Float, anchor: Float)])
        -> [(id: String, score: Float, anchor: Float)] {
        Dictionary(grouping: candidates, by: \.id)
            .compactMap { $0.value.max { left, right in left.score < right.score } }
            .sorted { left, right in left.score > right.score }
    }

    private func weightedAverage(_ windows: [SpeakerEmbeddingWindow]) -> [Float] {
        let totalWeight = max(windows.reduce(Float.zero) { $0 + max($1.weight, 0.01) }, 0.01)
        var result = Array(repeating: Float.zero, count: 256)
        for window in windows {
            let weight = max(window.weight, 0.01) / totalWeight
            for index in result.indices { result[index] += window.embedding[index] * weight }
        }
        return result
    }

    private func enrollOrRememberCandidate(
        from windows: [SpeakerEmbeddingWindow],
        generation: Int,
        consistencyThreshold: Float,
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?
    ) throws -> (String?, String?, SpeakerIdentificationStatusValue) {
        guard let candidate = windows.first else { return (nil, nil, .unknown) }
        let now = Date()
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        _ = try withAuthorizedStateChange(canCommit: canCommit, authorizeCommit: authorizeCommit) {
            pendingCandidates.removeAll {
                now.timeIntervalSince($0.observedAt) > speakerPendingCandidateLifetimeSeconds
            }
        }
        if let index = pendingCandidates.indices.max(by: {
            cosineSimilarity(pendingCandidates[$0].embedding, candidate.embedding)
                < cosineSimilarity(pendingCandidates[$1].embedding, candidate.embedding)
        }), cosineSimilarity(pendingCandidates[index].embedding, candidate.embedding)
            >= consistencyThreshold {
            let previous = pendingCandidates[index]
            let embedding = try normalizedEmbedding(weightedAverage([
                SpeakerEmbeddingWindow(
                    embedding: previous.embedding,
                    startSample: 0,
                    weight: previous.weight
                ),
                candidate,
            ]))
            let id = try register(
                embedding: embedding,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            // register が成功した場合は commit 済みなので、ここで deadline を再確認して
            // unavailable に戻すと、永続化済みの ID と応答が不一致になる。
            pendingCandidates.remove(at: index)
            return (id, document?.registryID, .identified)
        }
        _ = try withAuthorizedStateChange(canCommit: canCommit, authorizeCommit: authorizeCommit) {
            pendingCandidates.append(
                PendingSpeakerCandidate(
                    generation: generation,
                    embedding: candidate.embedding,
                    weight: candidate.weight,
                    observedAt: now
                )
            )
            if pendingCandidates.count > speakerMaximumPendingCandidates {
                pendingCandidates.removeFirst(pendingCandidates.count - speakerMaximumPendingCandidates)
            }
        }
        return (nil, nil, .unknown)
    }

    private func withAuthorizedStateChange<T>(
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?,
        _ body: () throws -> T
    ) throws -> T {
        if let authorizeCommit {
            guard let permit = authorizeCommit() else {
                throw SpeakerIdentificationFailure.deadlineExceeded
            }
            defer { permit.release() }
            guard permit.isActive() else {
                throw SpeakerIdentificationFailure.deadlineExceeded
            }
            afterCommitReserved()
            return try permit.withActiveReservation(body)
        }
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        return try body()
    }

    private func nonOverlappingWindows(_ windows: [SpeakerEmbeddingWindow]) -> [SpeakerEmbeddingWindow] {
        var selected: [SpeakerEmbeddingWindow] = []
        for window in windows.sorted(by: { $0.startSample < $1.startSample }) {
            guard selected.last.map({
                window.startSample >= $0.startSample + speakerEvidenceWindowSampleCount
            }) ?? true else {
                continue
            }
            selected.append(window)
        }
        return selected
    }

    private func updateCentroid(
        id: String,
        embedding: [Float],
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?
    ) throws {
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        var document = try writableDocument()
        guard let index = document.profiles.firstIndex(where: { $0.id == canonicalID(id) }) else { return }
        let current = document.profiles[index].centroid
        let updated = try normalizedEmbedding(zip(current, embedding).map { 0.95 * $0.0 + 0.05 * $0.1 })
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        document.profiles[index].centroid = updated
        document.profiles[index].updateCount &+= 1
        document.revision &+= 1
        try save(
            document,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        self.document = document
    }

    private func register(
        embedding: [Float],
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?
    ) throws -> String {
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        guard let modelPackageDigest = expectedModelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        var document = document ?? SpeakerRegistryDocument(
            schemaVersion: 1,
            registryID: UUID().uuidString.lowercased(),
            nextSpeakerNumber: 1,
            modelIdentifier: speakerIdentificationModelIdentifier,
            modelPackageDigest: modelPackageDigest,
            preprocessingVersion: speakerIdentificationPreprocessingVersion,
            decisionVersion: speakerIdentificationDecisionVersion,
            profiles: [],
            aliases: [:],
            revision: 0
        )
        guard document.profiles.count < speakerMaximumProfiles else {
            throw SpeakerIdentificationFailure.ledgerWrite("話者台帳の上限に達しました")
        }
        guard document.modelPackageDigest == modelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        guard document.nextSpeakerNumber < UInt64.max else {
            throw SpeakerIdentificationFailure.ledgerWrite("話者 ID の連番上限に達しました")
        }
        let id = "speaker-\(document.nextSpeakerNumber)"
        document.nextSpeakerNumber += 1
        document.profiles.append(
            SpeakerProfile(
                id: id,
                anchor: embedding,
                centroid: embedding,
                updateCount: 0,
                state: "active"
            )
        )
        document.revision &+= 1
        try save(
            document,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        self.document = document
        return id
    }

    private func save(
        _ document: SpeakerRegistryDocument,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil
    ) throws {
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        let data: Data
        do {
            data = try JSONEncoder().encode(document)
        } catch {
            throw SpeakerIdentificationFailure.ledgerWrite(error.localizedDescription)
        }
        guard data.count <= speakerMaximumRegistryBytes else {
            throw SpeakerIdentificationFailure.ledgerWrite("話者台帳の上限を超えました")
        }
        let aliasIndex = SpeakerAliasIndex(
            schemaVersion: 1,
            registryID: document.registryID,
            aliases: document.aliases
        )
        let aliasData: Data
        do {
            aliasData = try JSONEncoder().encode(aliasIndex)
        } catch {
            throw SpeakerIdentificationFailure.ledgerWrite(error.localizedDescription)
        }
        let directory = path.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(
                at: directory,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            try FileManager.default.setAttributes(
                [.posixPermissions: 0o700],
                ofItemAtPath: directory.path
            )
        } catch {
            throw SpeakerIdentificationFailure.ledgerWrite(error.localizedDescription)
        }
        let descriptor = try acquireLock()
        defer {
            close(descriptor)
            unlink(lockPath.path)
        }
        do {
            try writeLockMetadata(to: descriptor)
            if let authorizeCommit {
                guard let acquired = authorizeCommit() else {
                    throw SpeakerIdentificationFailure.deadlineExceeded
                }
                defer { acquired.release() }
                beforePersistence()
                guard acquired.beginPersistence() else {
                    throw SpeakerIdentificationFailure.deadlineExceeded
                }
                afterPersistenceBegan()
            } else {
                guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
            }
            let key = try keyStore.readOrCreate()
            let sealed = try AES.GCM.seal(data, using: key)
            guard let combined = sealed.combined else {
                throw SpeakerIdentificationFailure.ledgerWrite("暗号化結果がありません")
            }
            let envelope = SpeakerRegistryEnvelope(
                schemaVersion: 1,
                combined: combined.base64EncodedString()
            )
            let envelopeData = try JSONEncoder().encode(envelope)
            guard envelopeData.count <= speakerMaximumRegistryBytes else {
                throw SpeakerIdentificationFailure.ledgerWrite("話者台帳の上限を超えました")
            }
            try Self.atomicWrite(envelopeData, to: path)
            do {
                if shouldFailAliasWrite() {
                    throw SpeakerIdentificationFailure.ledgerAliasWrite(
                        "テスト用に別名索引の保存を失敗させました"
                    )
                }
                try Self.atomicWrite(aliasData, to: aliasesPath)
            } catch {
                let failure = aliasWriteFailure(error)
                identificationFailure = failure
                throw failure
            }
            try synchronizeDirectory(directory)
        } catch {
            if let error = error as? SpeakerIdentificationFailure {
                throw error
            }
            throw SpeakerIdentificationFailure.ledgerWrite(error.localizedDescription)
        }
    }

    private func acquireLock() throws -> Int32 {
        func openLock() -> Int32 {
            open(lockPath.path, O_CREAT | O_EXCL | O_WRONLY, 0o600)
        }
        var descriptor = openLock()
        if descriptor < 0, errno == EEXIST {
            try Self.removeStaleLockIfNeeded(at: lockPath)
            descriptor = openLock()
        }
        guard descriptor >= 0 else { throw SpeakerIdentificationFailure.ledgerLocked }
        return descriptor
    }

    private func writeLockMetadata(to descriptor: Int32) throws {
        let data = Data("\(getpid()) \(Date().timeIntervalSince1970)\n".utf8)
        let written = data.withUnsafeBytes { buffer in
            Darwin.write(descriptor, buffer.baseAddress, data.count)
        }
        guard written == data.count, fsync(descriptor) == 0 else {
            throw SpeakerIdentificationFailure.ledgerWrite("台帳のロックを同期できません")
        }
    }

    private static func atomicWrite(_ data: Data, to url: URL) throws {
        try data.write(to: url, options: .atomic)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: url.path
        )
        let descriptor = open(url.path, O_RDONLY)
        guard descriptor >= 0 else {
            throw SpeakerIdentificationFailure.ledgerWrite("台帳を同期できません")
        }
        defer { close(descriptor) }
        guard fsync(descriptor) == 0 else {
            throw SpeakerIdentificationFailure.ledgerWrite("台帳を同期できません")
        }
    }

    private func synchronizeDirectory(_ directory: URL) throws {
        let descriptor = open(directory.path, O_RDONLY)
        guard descriptor >= 0 else {
            throw SpeakerIdentificationFailure.ledgerWrite("台帳の保存先を同期できません")
        }
        defer { close(descriptor) }
        guard fsync(descriptor) == 0 else {
            throw SpeakerIdentificationFailure.ledgerWrite("台帳の保存先を同期できません")
        }
    }
}

private func pairwiseCosineSimilarities(_ windows: [SpeakerEmbeddingWindow]) -> [Float] {
    guard windows.count > 1 else { return [] }
    var similarities: [Float] = []
    similarities.reserveCapacity(windows.count * (windows.count - 1) / 2)
    for leftIndex in 0..<(windows.count - 1) {
        for rightIndex in (leftIndex + 1)..<windows.count {
            similarities.append(
                cosineSimilarity(
                    windows[leftIndex].embedding,
                    windows[rightIndex].embedding
                )
            )
        }
    }
    return similarities
}

private struct SpeakerWindowClusterSelection {
    let primary: [Int]
    let secondary: [Int]
    let isMixed: Bool
}

private func speakerWindowClusterSelection(
    _ windows: [SpeakerEmbeddingWindow],
    consistencyThreshold: Float
) -> SpeakerWindowClusterSelection {
    let primary = largestSpeakerWindowCluster(
        in: windows,
        consistencyThreshold: consistencyThreshold
    )
    let secondary = largestSpeakerWindowCluster(
        in: windows,
        excluding: Set(primary),
        consistencyThreshold: consistencyThreshold
    )
    let primaryMinimum = max(
        1,
        Int(ceil(Double(windows.count) * speakerWindowPrimaryClusterFraction))
    )
    let secondaryMinimum = max(
        2,
        Int(ceil(Double(windows.count) * speakerWindowSecondaryClusterFraction))
    )
    return SpeakerWindowClusterSelection(
        primary: primary,
        secondary: secondary,
        isMixed: primary.count < primaryMinimum || secondary.count >= secondaryMinimum
    )
}

private func largestSpeakerWindowCluster(
    in windows: [SpeakerEmbeddingWindow],
    excluding excluded: Set<Int> = [],
    consistencyThreshold: Float
) -> [Int] {
    let candidates = windows.indices.filter { !excluded.contains($0) }
    var best: [Int] = []

    func isBetter(_ candidate: [Int], than current: [Int]) -> Bool {
        guard candidate.count == current.count else {
            return candidate.count > current.count
        }
        let candidateWeight = candidate.reduce(Float.zero) {
            $0 + max(windows[$1].weight, 0.01)
        }
        let currentWeight = current.reduce(Float.zero) {
            $0 + max(windows[$1].weight, 0.01)
        }
        if candidateWeight != currentWeight {
            return candidateWeight > currentWeight
        }
        let candidateStart = candidate.map { windows[$0].startSample }.min() ?? Int.max
        let currentStart = current.map { windows[$0].startSample }.min() ?? Int.max
        return candidateStart < currentStart
    }

    func search(_ cluster: [Int], _ remaining: [Int]) {
        guard cluster.count + remaining.count >= best.count else { return }
        if isBetter(cluster, than: best) { best = cluster }
        guard !remaining.isEmpty else { return }

        var remaining = remaining
        while let candidate = remaining.popLast() {
            let compatible = cluster.allSatisfy {
                cosineSimilarity(
                    windows[$0].embedding,
                    windows[candidate].embedding
                ) >= consistencyThreshold
            }
            guard compatible else { continue }
            let next = remaining.filter {
                cosineSimilarity(
                    windows[candidate].embedding,
                    windows[$0].embedding
                ) >= consistencyThreshold
            }
            search(cluster + [candidate], next)
        }
    }

    search([], candidates)
    return best.sorted()
}

private func speakerWindowSampleCounts(for sampleRate: Double) throws
    -> (window: Int, interval: Int) {
    guard sampleRate.isFinite, sampleRate > 0 else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    let windowSamples = sampleRate * speakerWindowDurationSeconds
    guard windowSamples.isFinite,
          windowSamples > 0,
          windowSamples <= Double(Int.max),
          sampleRate <= Double(Int.max) else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    let window = Int(windowSamples.rounded())
    let interval = Int(sampleRate.rounded())
    guard window > 0, interval > 0 else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    return (window: window, interval: interval)
}

private func makeSpeakerEmbeddingWindow(
    from samples: [Float],
    sampleRate: Double,
    sourceStart: Int,
    predictor: SpeakerEmbeddingPredictor
) throws -> SpeakerEmbeddingWindow? {
    let sampleCounts = try speakerWindowSampleCounts(for: sampleRate)
    guard sourceStart >= 0,
          sourceStart <= samples.count,
          sampleCounts.window <= samples.count - sourceStart else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    let sourceWindow = Array(
        samples[sourceStart..<(sourceStart + sampleCounts.window)]
    )
    let window = try speakerResample(sourceWindow, from: sampleRate)
    let speechSamples = speakerSpeechSampleCount(window)
    let peak = window.map { abs($0) }.max() ?? 0
    guard speechSamples >= speakerMinimumSpeechSamples, peak < 1.0 else {
        return nil
    }
    let features = try WeSpeakerFbank.features(for: window)
    let embedding = try predictor.predict(features: features)
    return SpeakerEmbeddingWindow(
        embedding: embedding,
        startSample: Int(
            (Double(sourceStart) * speakerSampleRate / sampleRate).rounded()
        ),
        weight: Float(speechSamples) / Float(speakerWindowSampleCount)
    )
}

private func speakerEmbeddingWindows(
    from samples: [Float],
    sampleRate: Double,
    predictor: SpeakerEmbeddingPredictor
) throws -> [SpeakerEmbeddingWindow] {
    let sampleCounts = try speakerWindowSampleCounts(for: sampleRate)
    guard samples.count >= sampleCounts.window else { return [] }
    var windows: [SpeakerEmbeddingWindow] = []
    var sourceStart = 0
    while sourceStart <= samples.count - sampleCounts.window {
        if let window = try makeSpeakerEmbeddingWindow(
            from: samples,
            sampleRate: sampleRate,
            sourceStart: sourceStart,
            predictor: predictor
        ) {
            windows.append(window)
        }
        sourceStart += sampleCounts.interval
    }
    return windows
}

private func speakerSpeechSampleCount(_ samples: [Float]) -> Int {
    let frameCount = samples.count / speakerFrameShiftSampleCount
    var count = 0
    for frame in 0..<frameCount {
        let start = frame * speakerFrameShiftSampleCount
        let rms = sqrt(
            samples[start..<(start + speakerFrameShiftSampleCount)]
                .reduce(Float.zero) { $0 + $1 * $1 }
                / Float(speakerFrameShiftSampleCount)
        )
        if rms >= 0.005 { count += speakerFrameShiftSampleCount }
    }
    return count
}

final class SpeakerIdentificationCoordinator {
    private struct SegmentMetadata {
        let segmentID: String
        let audioStartNanoseconds: UInt64
        let sampleRate: Double
    }

    private var predictor: SpeakerEmbeddingPredictor?
    private let ledger: SpeakerLedger
    private let worker = DispatchQueue(
        label: "dev.nrslib.coosenpai.hearing.speaker-identification",
        qos: .userInitiated
    )
    private let workerQueueKey = DispatchSpecificKey<Void>()
    private let stateLock = NSLock()
    private let log: (String) -> Void
    private let preparationStatus: (SpeakerIdentificationPreparationStatus) -> Void
    private var preparationState: SpeakerIdentificationPreparationStatus = .preparing
    private var preparationFailure: SpeakerIdentificationFailure?
    private var preparationCancelled = false
    private var segment: SpeakerSegment?
    private var currentGeneration: Int?
    private var metadata: [Int: SegmentMetadata] = [:]
    private var cancelledGenerations: Set<Int> = []
    private var rejectedGenerations: Set<Int> = []
    private let commitState = SpeakerCommitStateMachine()
    private var pendingBufferCount = 0
    private let maximumPendingBuffers = 8
    private let finishTimeout: DispatchTimeInterval = .milliseconds(500)

    init(
        modelPath: String?,
        ledgerPath: String,
        log: @escaping (String) -> Void,
        status: @escaping (SpeakerIdentificationPreparationStatus) -> Void = { _ in }
    ) throws {
        guard let modelPath else { throw SpeakerIdentificationFailure.modelPathMissing }
        guard !modelPath.isEmpty else { throw SpeakerIdentificationFailure.modelPathMissing }
        let modelURL = URL(fileURLWithPath: modelPath, isDirectory: modelPath.hasSuffix(".mlpackage"))
        guard FileManager.default.fileExists(atPath: modelURL.path) else {
            throw SpeakerIdentificationFailure.modelFileMissing(modelURL.path)
        }
        guard modelURL.pathExtension == "mlpackage" else {
            throw SpeakerIdentificationFailure.modelLoad(
                "検証済みの Core ML .mlpackage だけを指定できます"
            )
        }
        ledger = try SpeakerLedger(path: ledgerPath)
        self.log = log
        self.preparationStatus = status
        worker.setSpecific(key: workerQueueKey, value: ())
        log("speaker-identification model status=preparing")
        status(.preparing)
        let cacheDirectory = speakerModelCacheDirectory(for: ledgerPath)
        worker.async { [weak self] in
            self?.prepareModel(path: modelPath, cacheDirectory: cacheDirectory)
        }
    }

    init(
        forTesting predictor: SpeakerEmbeddingPredictor,
        ledgerPath: String,
        keyStore: SpeakerKeyStore,
        modelPackageDigest: String,
        preparationState: SpeakerIdentificationPreparationStatus = .ready,
        persistenceDelay: TimeInterval = 0,
        persistenceDelayAfterBegin: TimeInterval = 0,
        persistenceReserved: @escaping () -> Void = {},
        persistenceBegan: @escaping () -> Void = {},
        commitReserved: @escaping () -> Void = {},
        shouldFailAliasWrite: @escaping () -> Bool = { false },
        log: @escaping (String) -> Void = { _ in }
    ) throws {
        ledger = try SpeakerLedger(
            path: ledgerPath,
            keyStore: keyStore,
            modelPackageDigest: modelPackageDigest,
            beforePersistence: {
                persistenceReserved()
                if persistenceDelay > 0 {
                    Thread.sleep(forTimeInterval: persistenceDelay)
                }
            },
            afterPersistenceBegan: {
                persistenceBegan()
                if persistenceDelayAfterBegin > 0 {
                    Thread.sleep(forTimeInterval: persistenceDelayAfterBegin)
                }
            },
            afterCommitReserved: commitReserved,
            shouldFailAliasWrite: shouldFailAliasWrite
        )
        self.log = log
        preparationStatus = { _ in }
        worker.setSpecific(key: workerQueueKey, value: ())
        self.preparationState = preparationState
        if preparationState == .ready {
            self.predictor = predictor
        } else if preparationState == .unavailable {
            self.preparationFailure = .modelUnavailable
        }
    }

    private func prepareModel(path: String, cacheDirectory: URL) {
        let startedAt = DispatchTime.now().uptimeNanoseconds
        do {
            let predictor = try CoreMLSpeakerEmbeddingPredictor(
                path: path,
                cacheDirectory: cacheDirectory
            )
            guard !isPreparationCancelled() else { return }
            try ledger.bindModelPackageDigest(predictor.modelPackageDigest)
            guard !isPreparationCancelled() else { return }
            stateLock.lock()
            self.predictor = predictor
            preparationFailure = nil
            preparationState = .ready
            stateLock.unlock()
            let elapsedMilliseconds =
                (DispatchTime.now().uptimeNanoseconds - startedAt) / 1_000_000
            let cache = predictor.modelCacheHit ? "hit" : "miss"
            log(
                "speaker-identification model status=ready cache=\(cache) "
                    + "elapsed-ms=\(elapsedMilliseconds)"
            )
            preparationStatus(.ready)
        } catch let error as SpeakerIdentificationFailure {
            guard !isPreparationCancelled() else { return }
            stateLock.lock()
            preparationFailure = error
            preparationState = .unavailable
            stateLock.unlock()
            log("speaker-identification unavailable reason=\(error.localizedDescription)")
            preparationStatus(.unavailable)
        } catch {
            guard !isPreparationCancelled() else { return }
            let failure = SpeakerIdentificationFailure.modelLoad(error.localizedDescription)
            stateLock.lock()
            preparationFailure = failure
            preparationState = .unavailable
            stateLock.unlock()
            log("speaker-identification unavailable reason=\(failure.localizedDescription)")
            preparationStatus(.unavailable)
        }
    }

    private func markUnavailable(_ failure: SpeakerIdentificationFailure) {
        stateLock.lock()
        guard !preparationCancelled else {
            stateLock.unlock()
            return
        }
        preparationFailure = failure
        preparationState = .unavailable
        stateLock.unlock()
        preparationStatus(.unavailable)
    }

    func beginSegment(generation: Int, sampleRate: Double, audioStartNanoseconds: UInt64) {
        let segmentID = UUID().uuidString.lowercased()
        stateLock.lock()
        guard !preparationCancelled,
              let generationStart = commitState.startGeneration(generation) else {
            stateLock.unlock()
            return
        }
        let previousGeneration = generationStart.previousGeneration
        let preparationFailure: SpeakerIdentificationFailure?
        switch self.preparationState {
        case .preparing:
            preparationFailure = .modelPreparing
        case .ready:
            preparationFailure = nil
        case .unavailable:
            preparationFailure = self.preparationFailure ?? .modelUnavailable
        }
        currentGeneration = generation
        metadata[generation] = SegmentMetadata(
            segmentID: segmentID,
            audioStartNanoseconds: audioStartNanoseconds,
            sampleRate: sampleRate
        )
        if let previousGeneration, previousGeneration != generation {
            cancelledGenerations.insert(previousGeneration)
        }
        if commitState.isGenerationActive(generation) {
            cancelledGenerations.remove(generation)
            rejectedGenerations.remove(generation)
        } else {
            cancelledGenerations.insert(generation)
        }
        stateLock.unlock()
        if let previousGeneration, previousGeneration != generation {
            worker.async { [weak self] in
                self?.ledger.discardPendingCandidates(for: previousGeneration)
            }
        }
        guard commitState.isGenerationActive(generation) else {
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
            return
        }
        worker.async { [weak self] in
            guard let self, !self.isCancelled(generation) else { return }
            var newSegment = SpeakerSegment(
                generation: generation,
                segmentID: segmentID,
                audioStartNanoseconds: audioStartNanoseconds,
                sampleRate: sampleRate
            )
            if !sampleRate.isFinite || sampleRate <= 0 {
                newSegment.failure = .modelInputUnavailable
            } else if let preparationFailure {
                newSegment.failure = preparationFailure
            }
            self.segment = newSegment
        }
    }

    func append(_ buffer: AVAudioPCMBuffer, generation: Int) {
        let copiedBuffer: AVAudioPCMBuffer
        do {
            copiedBuffer = try deepCopyAudioBuffer(buffer)
        } catch {
            reject(generation: generation)
            return
        }
        stateLock.lock()
        guard currentGeneration == generation,
              !cancelledGenerations.contains(generation),
              !rejectedGenerations.contains(generation),
              commitState.isGenerationActive(generation),
              pendingBufferCount < maximumPendingBuffers else {
            if currentGeneration == generation {
                rejectedGenerations.insert(generation)
            }
            stateLock.unlock()
            return
        }
        pendingBufferCount += 1
        stateLock.unlock()
        worker.async { [weak self, copiedBuffer] in
            guard let self else { return }
            defer {
                self.stateLock.lock()
                self.pendingBufferCount = max(0, self.pendingBufferCount - 1)
                self.stateLock.unlock()
            }
            guard !self.isCancelled(generation), var segment = self.segment,
                  segment.generation == generation,
                  segment.failure == nil else { return }
            do {
                let mono = try monoFloat32AudioBuffer(from: copiedBuffer)
                guard let data = mono.floatChannelData?[0] else {
                    throw SpeakerIdentificationFailure.modelInputUnavailable
                }
                segment.samples.append(contentsOf: UnsafeBufferPointer(
                    start: data,
                    count: Int(mono.frameLength)
                ))
                if segment.samples.count > Int(segment.sampleRate * 15.5) {
                    segment.failure = .modelInputUnavailable
                } else {
                    try self.processAvailableWindows(&segment)
                }
            } catch let error as SpeakerIdentificationFailure {
                segment.failure = error
            } catch {
                segment.failure = .modelInputUnavailable
            }
            guard !self.isCancelled(generation) else { return }
            self.segment = segment
        }
    }

    func finishSegment(generation: Int, audioEndNanoseconds: UInt64) -> SpeakerIdentificationResult {
        let metadata = segmentMetadata(for: generation)
        let fallback = unavailableResult(
            segmentID: metadata?.segmentID ?? UUID().uuidString.lowercased(),
            audioStartNanoseconds: metadata?.audioStartNanoseconds ?? 0,
            audioEndNanoseconds: max(
                audioEndNanoseconds,
                (metadata?.audioStartNanoseconds ?? 0) &+ 1
            )
        )
        stateLock.lock()
        let shouldReturnUnavailable = currentGeneration != generation
            || cancelledGenerations.contains(generation)
            || rejectedGenerations.contains(generation)
            || !commitState.isGenerationActive(generation)
        if shouldReturnUnavailable {
            cancelledGenerations.insert(generation)
            commitState.cancelGeneration(generation)
        }
        stateLock.unlock()
        if shouldReturnUnavailable {
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
            return fallback
        }

        let deadline = DispatchTime.now() + finishTimeout
        let semaphore = DispatchSemaphore(value: 0)
        var result = fallback
        worker.async { [weak self] in
            guard let self else {
                semaphore.signal()
                return
            }
            defer {
                semaphore.signal()
            }
            guard !self.isCancelled(generation),
                  let segment = self.segment,
                  segment.generation == generation else {
                return
            }
            guard self.canCommit(generation: generation, before: deadline) else { return }
            self.segment = nil
            let start = segment.audioStartNanoseconds
            let end = max(
                audioEndNanoseconds,
                max(segment.audioEndNanoseconds, start &+ 1)
            )
            if let failure = segment.failure {
                self.log(
                    "speaker-identification disabled generation=\(generation) reason=\(failure.localizedDescription)"
                )
                result = self.unavailableResult(
                    segmentID: segment.segmentID,
                    audioStartNanoseconds: start,
                    audioEndNanoseconds: end
                )
                return
            }
            self.stateLock.lock()
            let preparationState = self.preparationState
            let preparationFailure = self.preparationFailure
            let hasPredictor = self.predictor != nil
            self.stateLock.unlock()
            if preparationState != .ready || !hasPredictor {
                let failure = preparationFailure
                    ?? (preparationState == .preparing
                        ? SpeakerIdentificationFailure.modelPreparing
                        : SpeakerIdentificationFailure.modelUnavailable)
                self.log(
                    "speaker-identification disabled generation=\(generation) reason=\(failure.localizedDescription)"
                )
                result = self.unavailableResult(
                    segmentID: segment.segmentID,
                    audioStartNanoseconds: start,
                    audioEndNanoseconds: end
                )
                return
            }
            do {
                let decision = try self.ledger.identify(
                    windows: segment.windows,
                    generation: generation,
                    canCommit: {
                        self.canCommit(generation: generation, before: deadline)
                    },
                    authorizeCommit: {
                        self.authorizeCommit(generation: generation, before: deadline)
                    }
                )
                result = SpeakerIdentificationResult(
                    segmentID: segment.segmentID,
                    audioStartMilliseconds: start / 1_000_000,
                    audioEndMilliseconds: max(end / 1_000_000, start / 1_000_000 + 1),
                    speakerID: decision.0,
                    registryID: decision.1,
                    status: decision.2
                )
            } catch {
                if let failure = error as? SpeakerIdentificationFailure,
                   failure.stopsIdentification {
                    self.markUnavailable(failure)
                }
                self.log(
                    "speaker-identification unavailable generation=\(generation) reason=\(error.localizedDescription)"
                )
                result = self.unavailableResult(
                    segmentID: segment.segmentID,
                    audioStartNanoseconds: start,
                    audioEndNanoseconds: end
                )
            }
        }
        guard semaphore.wait(timeout: deadline) == .success else {
            if let reservation = commitState.reservation(for: generation) {
                if reservation.cancel() {
                    invalidateGeneration(generation)
                    clearSegmentLater(generation: generation, discardPendingCandidates: true)
                    log(
                        "speaker-identification unavailable generation=\(generation) "
                            + "reason=deadline-exceeded reservation-cancelled"
                    )
                    return fallback
                }
                if reservation.isPersisting() {
                    // 永続化完了は worker 側で受ける。音声処理キューでは待たず、
                    // この区間の結果を先に unavailable として返す。
                    invalidateGeneration(generation)
                    clearSegmentLater(generation: generation, discardPendingCandidates: true)
                    log(
                        "speaker-identification unavailable generation=\(generation) "
                            + "reason=deadline-exceeded persistence-in-progress"
                    )
                    return fallback
                }
            }
            invalidateGeneration(generation)
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
            log("speaker-identification unavailable generation=\(generation) reason=deadline-exceeded")
            return fallback
        }
        guard commitState.isGenerationActive(generation) else {
            invalidateGeneration(generation)
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
            log(
                "speaker-identification unavailable generation=\(generation) "
                    + "reason=cancelled-before-result"
            )
            return fallback
        }
        stateLock.lock()
        guard currentGeneration == generation,
              !cancelledGenerations.contains(generation),
              !rejectedGenerations.contains(generation),
              commitState.isGenerationActive(generation) else {
            cancelledGenerations.insert(generation)
            commitState.cancelGeneration(generation)
            stateLock.unlock()
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
            log(
                "speaker-identification unavailable generation=\(generation) "
                    + "reason=cancelled-before-publication"
            )
            return fallback
        }
        if currentGeneration == generation { currentGeneration = nil }
        self.metadata.removeValue(forKey: generation)
        cancelledGenerations.remove(generation)
        rejectedGenerations.remove(generation)
        commitState.finishGeneration(generation)
        stateLock.unlock()
        return result
    }

    func cancel() {
        stateLock.lock()
        guard let generation = commitState.cancelCurrentGeneration() else {
            stateLock.unlock()
            return
        }
        cancelledGenerations.insert(generation)
        stateLock.unlock()
        clearSegmentLater(generation: generation, discardPendingCandidates: true)
    }

    func shutdown() {
        stateLock.lock()
        preparationCancelled = true
        let generations = Set(metadata.keys).union(currentGeneration.map { [$0] } ?? [])
        for generation in generations {
            cancelledGenerations.insert(generation)
        }
        commitState.shutdown()
        stateLock.unlock()
        for generation in generations {
            clearSegmentLater(generation: generation, discardPendingCandidates: true)
        }
        if DispatchQueue.getSpecific(key: workerQueueKey) == nil {
            worker.sync {}
        }
    }

    private func segmentMetadata(for generation: Int) -> SegmentMetadata? {
        stateLock.lock()
        defer { stateLock.unlock() }
        return metadata[generation]
    }

    private func isCancelled(_ generation: Int) -> Bool {
        stateLock.lock()
        let cancelled = cancelledGenerations.contains(generation)
        stateLock.unlock()
        return cancelled || !commitState.isGenerationActive(generation)
    }

    private func isPreparationCancelled() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return preparationCancelled
    }

    private func canCommit(generation: Int, before deadline: DispatchTime) -> Bool {
        let now = DispatchTime.now()
        guard now < deadline,
              deadline.uptimeNanoseconds - now.uptimeNanoseconds
                  > speakerLedgerCommitSafetyNanoseconds else {
            return false
        }
        return commitState.isGenerationActive(generation)
    }

    private func authorizeCommit(
        generation: Int,
        before deadline: DispatchTime
    ) -> SpeakerLedgerCommitPermit? {
        let now = DispatchTime.now()
        guard now < deadline,
              deadline.uptimeNanoseconds - now.uptimeNanoseconds
                  > speakerLedgerCommitSafetyNanoseconds else {
            return nil
        }
        return commitState.reserve(generation: generation, deadline: deadline)
    }

    private func reject(generation: Int) {
        stateLock.lock()
        if currentGeneration == generation { rejectedGenerations.insert(generation) }
        stateLock.unlock()
    }

    private func invalidateGeneration(_ generation: Int) {
        stateLock.lock()
        cancelledGenerations.insert(generation)
        commitState.cancelGeneration(generation)
        stateLock.unlock()
    }

    private func clearSegmentLater(
        generation: Int,
        discardPendingCandidates: Bool
    ) {
        worker.async { [weak self] in
            guard let self else { return }
            if self.segment?.generation == generation { self.segment = nil }
            if discardPendingCandidates {
                self.ledger.discardPendingCandidates(for: generation)
            }
            self.stateLock.lock()
            if self.currentGeneration == generation { self.currentGeneration = nil }
            self.metadata.removeValue(forKey: generation)
            self.rejectedGenerations.remove(generation)
            self.cancelledGenerations.remove(generation)
            self.stateLock.unlock()
        }
    }

    private func clearGeneration(_ generation: Int) {
        stateLock.lock()
        if currentGeneration == generation { currentGeneration = nil }
        metadata.removeValue(forKey: generation)
        cancelledGenerations.remove(generation)
        rejectedGenerations.remove(generation)
        commitState.finishGeneration(generation)
        stateLock.unlock()
    }

    private func processAvailableWindows(_ segment: inout SpeakerSegment) throws {
        stateLock.lock()
        let preparationState = self.preparationState
        let preparationFailure = self.preparationFailure
        let predictor = self.predictor
        stateLock.unlock()
        guard preparationState == .ready, let predictor else {
            if preparationState == .preparing { throw SpeakerIdentificationFailure.modelPreparing }
            throw preparationFailure ?? SpeakerIdentificationFailure.modelUnavailable
        }
        let sampleCounts = try speakerWindowSampleCounts(for: segment.sampleRate)
        while segment.samples.count >= sampleCounts.window,
              segment.nextWindowStartSample <= segment.samples.count - sampleCounts.window {
            let sourceStart = segment.nextWindowStartSample
            if let window = try makeSpeakerEmbeddingWindow(
                from: segment.samples,
                sampleRate: segment.sampleRate,
                sourceStart: sourceStart,
                predictor: predictor
            ) {
                segment.windows.append(
                    window
                )
            }
            segment.nextWindowStartSample += sampleCounts.interval
        }
    }

    private func unavailableResult(
        segmentID: String,
        audioStartNanoseconds: UInt64,
        audioEndNanoseconds: UInt64
    ) -> SpeakerIdentificationResult {
        SpeakerIdentificationResult(
            segmentID: segmentID,
            audioStartMilliseconds: audioStartNanoseconds / 1_000_000,
            audioEndMilliseconds: max(
                audioEndNanoseconds / 1_000_000,
                audioStartNanoseconds / 1_000_000 + 1
            ),
            speakerID: nil,
            registryID: nil,
            status: .unavailable
        )
    }
}

func speakerResample(_ samples: [Float], from sampleRate: Double) throws -> [Float] {
    guard sampleRate.isFinite, sampleRate > 0, !samples.isEmpty else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    let originalRate = Int(sampleRate.rounded())
    guard originalRate > 0, abs(sampleRate - Double(originalRate)) < 0.01 else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    let targetRate = Int(speakerSampleRate)
    if originalRate == targetRate {
        guard samples.count >= speakerWindowSampleCount else {
            throw SpeakerIdentificationFailure.modelInputUnavailable
        }
        return Array(samples.prefix(speakerWindowSampleCount))
    }

    // torchaudio.functional.resample の sinc_interp_hann、
    // lowpass_filter_width=6、rolloff=0.99 と同じ固定係数を使う。
    let divisor = greatestCommonDivisor(originalRate, targetRate)
    let reducedOriginalRate = originalRate / divisor
    let reducedTargetRate = targetRate / divisor
    let lowpassFilterWidth = Float(6.0)
    let baseFrequency = Float(min(reducedOriginalRate, reducedTargetRate)) * 0.99
    let width = Int(ceil(Double(lowpassFilterWidth * Float(reducedOriginalRate)) / Double(baseFrequency)))
    let indexRange = (-width)..<(width + reducedOriginalRate)
    let kernelWidth = indexRange.count
    let scale = baseFrequency / Float(reducedOriginalRate)
    let pi = Float.pi
    let kernels = (0..<reducedTargetRate).map { phase in
        indexRange.map { index in
            var time = -Float(phase) / Float(reducedTargetRate)
                + Float(index) / Float(reducedOriginalRate)
            time *= baseFrequency
            time = max(-lowpassFilterWidth, min(lowpassFilterWidth, time))
            let windowValue = cosf(time * pi / lowpassFilterWidth / 2.0)
            let window = windowValue * windowValue
            let argument = time * pi
            let sinc = abs(argument) < 1e-12 ? Float(1.0) : sinf(argument) / argument
            return sinc * window * scale
        }
    }
    let outputCount = Int(
        ceil(Double(samples.count) * Double(reducedTargetRate) / Double(reducedOriginalRate))
    )
    // WeSpeaker の公式推論は normalize=False の PCM 値でリサンプルする。
    // helper の入力は [-1, 1] の Float なので、係数演算の丸めも同じ尺度にする。
    let resampleInput = samples.map { $0 * 32_768.0 }
    let padded = Array(repeating: Float.zero, count: width)
        + resampleInput
        + Array(repeating: Float.zero, count: width + reducedOriginalRate)
    var output = Array(repeating: Float.zero, count: outputCount)
    for outputIndex in output.indices {
        let phase = outputIndex % reducedTargetRate
        let timeIndex = outputIndex / reducedTargetRate
        let sourceStart = timeIndex * reducedOriginalRate
        guard sourceStart + kernelWidth <= padded.count else {
            throw SpeakerIdentificationFailure.modelInputUnavailable
        }
        var value: Float = 0
        padded.withUnsafeBufferPointer { paddedBuffer in
            kernels[phase].withUnsafeBufferPointer { kernelBuffer in
                vDSP_dotpr(
                    paddedBuffer.baseAddress!.advanced(by: sourceStart),
                    1,
                    kernelBuffer.baseAddress!,
                    1,
                    &value,
                    vDSP_Length(kernelWidth)
                )
            }
        }
        output[outputIndex] = value
    }
    // 44.1 kHz と 22.05 kHz は ceil により1サンプル多くなるため、
    // fbank の入力契約へ合わせて余分なサンプルを切り詰める。
    guard output.count >= speakerWindowSampleCount else {
        throw SpeakerIdentificationFailure.modelInputUnavailable
    }
    return output.prefix(speakerWindowSampleCount).map { $0 / 32_768.0 }
}

private func greatestCommonDivisor(_ left: Int, _ right: Int) -> Int {
    var left = abs(left)
    var right = abs(right)
    while right != 0 {
        (left, right) = (right, left % right)
    }
    return max(left, 1)
}

func speakerManagementLedger(path: String) throws -> SpeakerLedger {
    try SpeakerLedger(path: path)
}

private let speakerManagementOutputLock = NSLock()

private func emitSpeakerManagement(_ value: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: value) else { return }
    speakerManagementOutputLock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
    speakerManagementOutputLock.unlock()
}

func runSpeakerManagementCommandIfRequested() -> Bool {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.contains("--speaker-management") else { return false }

    var operation: String?
    var ledgerPath: String?
    var sourceID: String?
    var targetID: String?
    var speakerID: String?
    var index = 0
    while index < arguments.count {
        switch arguments[index] {
        case "--speaker-management":
            guard operation == nil, index + 1 < arguments.count else {
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "話者管理操作を一度だけ指定してください",
                ])
                exit(2)
            }
            operation = arguments[index + 1]
            index += 2
        case "--speaker-ledger":
            guard ledgerPath == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "話者台帳は --speaker-ledger <path> で指定してください",
                ])
                exit(2)
            }
            ledgerPath = arguments[index + 1]
            index += 2
        case "--speaker-from":
            guard sourceID == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "統合元は --speaker-from <speaker-N> で指定してください",
                ])
                exit(2)
            }
            sourceID = arguments[index + 1]
            index += 2
        case "--speaker-to":
            guard targetID == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "統合先は --speaker-to <speaker-N> で指定してください",
                ])
                exit(2)
            }
            targetID = arguments[index + 1]
            index += 2
        case "--speaker-id":
            guard speakerID == nil,
                  index + 1 < arguments.count,
                  !arguments[index + 1].isEmpty,
                  !arguments[index + 1].hasPrefix("--") else {
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "話者 ID は --speaker-id <speaker-N> で指定してください",
                ])
                exit(2)
            }
            speakerID = arguments[index + 1]
            index += 2
        default:
            emitSpeakerManagement([
                "event": "error",
                "kind": "arguments",
                "message": "未対応の話者管理引数です: \(arguments[index])",
            ])
            exit(2)
        }
    }

    guard let operation, let ledgerPath else {
        emitSpeakerManagement([
            "event": "error",
            "kind": "arguments",
            "message": "話者管理には操作と --speaker-ledger <path> が必要です",
        ])
        exit(2)
    }
    do {
        let ledger = try speakerManagementLedger(path: ledgerPath)
        switch operation {
        case "merge":
            guard let sourceID, let targetID, speakerID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "統合には --speaker-from と --speaker-to が必要です"
                )
            }
            try ledger.merge(from: sourceID, to: targetID)
        case "undo-merge":
            guard let sourceID, targetID == nil, speakerID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "統合の取り消しには --speaker-from が必要です"
                )
            }
            try ledger.undoMerge(source: sourceID)
        case "reregister":
            guard let speakerID, sourceID == nil, targetID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "再登録には --speaker-id が必要です"
                )
            }
            try ledger.reregister(id: speakerID)
        case "delete":
            guard let speakerID, sourceID == nil, targetID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "削除には --speaker-id が必要です"
                )
            }
            try ledger.delete(id: speakerID)
        case "delete-all":
            guard sourceID == nil, targetID == nil, speakerID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "全削除には話者 ID を指定できません"
                )
            }
            try ledger.deleteAll()
        default:
            emitSpeakerManagement([
                "event": "error",
                "kind": "arguments",
                "message": "未対応の話者管理操作です: \(operation)",
            ])
            exit(2)
        }
        emitSpeakerManagement([
            "event": "speaker-management",
            "operation": operation,
        ])
        exit(0)
    } catch {
        emitSpeakerManagement([
            "event": "error",
            "kind": "speaker-identification",
            "message": error.localizedDescription,
        ])
        exit(1)
    }
}

private let speakerDiagnosticOutputLock = NSLock()

private func emitSpeakerDiagnostic(_ message: String) {
    speakerDiagnosticOutputLock.lock()
    FileHandle.standardError.write(Data("\(message)\n".utf8))
    speakerDiagnosticOutputLock.unlock()
}

private struct SpeakerDiagnosticAudio {
    let samples: [Float]
    let sampleRate: Double
}

private struct SpeakerDiagnosticReferenceFile: Decodable {
    let path: String
    let sourceStartSample: Int
    let sourceSampleRate: Double
    let resampled: [Float]
    let features: [[Float]]
    let rawEmbedding: [Float]
    let normalizedEmbedding: [Float]
}

private struct SpeakerDiagnosticReference: Decodable {
    let schemaVersion: Int
    let files: [SpeakerDiagnosticReferenceFile]
}

private func speakerDiagnosticInputFiles(
    at path: String,
    microphone: Bool
) throws -> [URL] {
    guard !path.isEmpty else { throw SpeakerIdentificationFailure.diagnosisInputMissing }
    let url = URL(fileURLWithPath: path)
    var isDirectory: ObjCBool = false
    guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
        throw SpeakerIdentificationFailure.diagnosisInputMissing
    }
    guard isDirectory.boolValue else {
        guard url.pathExtension.lowercased() == "wav" else {
            throw SpeakerIdentificationFailure.diagnosisInputNotWav
        }
        return [url]
    }

    guard let enumerator = FileManager.default.enumerator(
        at: url,
        includingPropertiesForKeys: [.isRegularFileKey, .isSymbolicLinkKey],
        options: [.skipsHiddenFiles]
    ) else {
        throw SpeakerIdentificationFailure.diagnosisInputDirectoryUnreadable
    }
    let prefix = microphone ? "segment-microphone-" : "segment-speaker-"
    var matchingFiles: [URL] = []
    for case let entry as URL in enumerator {
        let name = entry.lastPathComponent.lowercased()
        guard name.hasPrefix(prefix),
              entry.pathExtension.lowercased() == "wav" else {
            continue
        }
        guard let values = try? entry.resourceValues(
            forKeys: [.isRegularFileKey, .isSymbolicLinkKey]
        ),
              values.isRegularFile == true,
              values.isSymbolicLink != true else {
            continue
        }
        matchingFiles.append(entry)
    }
    guard !matchingFiles.isEmpty else {
        throw SpeakerIdentificationFailure.diagnosisInputHasNoMatchingWav(prefix)
    }
    return matchingFiles.sorted { left, right in
        let leftName = left.deletingPathExtension().lastPathComponent
        let rightName = right.deletingPathExtension().lastPathComponent
        let leftNumber = Int(leftName.dropFirst(prefix.count)) ?? Int.max
        let rightNumber = Int(rightName.dropFirst(prefix.count)) ?? Int.max
        if leftNumber != rightNumber { return leftNumber < rightNumber }
        return left.path < right.path
    }
}

private func readSpeakerDiagnosticAudio(at url: URL) throws -> SpeakerDiagnosticAudio {
    let audioFile: AVAudioFile
    do {
        audioFile = try AVAudioFile(forReading: url)
    } catch {
        throw SpeakerIdentificationFailure.diagnosisInputRead(String(describing: error))
    }
    let sampleRate = audioFile.processingFormat.sampleRate
    guard sampleRate.isFinite, sampleRate > 0, audioFile.length > 0 else {
        throw SpeakerIdentificationFailure.diagnosisInputRead("音声フォーマットまたはフレーム数が不正です")
    }
    guard audioFile.length <= AVAudioFramePosition(Int.max) else {
        throw SpeakerIdentificationFailure.diagnosisInputRead("音声フレーム数が上限を超えています")
    }
    var samples: [Float] = []
    samples.reserveCapacity(Int(audioFile.length))
    let frameCapacity: AVAudioFrameCount = 4_096
    while audioFile.framePosition < audioFile.length {
        let remaining = audioFile.length - audioFile.framePosition
        let frameCount = AVAudioFrameCount(
            min(remaining, AVAudioFramePosition(frameCapacity))
        )
        guard frameCount > 0,
              let buffer = AVAudioPCMBuffer(
                  pcmFormat: audioFile.processingFormat,
                  frameCapacity: frameCount
              ) else {
            throw SpeakerIdentificationFailure.diagnosisInputRead("音声バッファを確保できません")
        }
        do {
            try audioFile.read(into: buffer, frameCount: frameCount)
        } catch {
            throw SpeakerIdentificationFailure.diagnosisInputRead(String(describing: error))
        }
        guard buffer.frameLength > 0 else { break }
        let mono: AVAudioPCMBuffer
        do {
            mono = try monoFloat32AudioBuffer(from: buffer)
        } catch {
            throw SpeakerIdentificationFailure.diagnosisInputRead(error.localizedDescription)
        }
        guard let data = mono.floatChannelData?[0] else {
            throw SpeakerIdentificationFailure.diagnosisInputRead("mono 音声データがありません")
        }
        samples.append(contentsOf: UnsafeBufferPointer(
            start: data,
            count: Int(mono.frameLength)
        ))
    }
    guard !samples.isEmpty else {
        throw SpeakerIdentificationFailure.diagnosisInputRead("音声フレームがありません")
    }
    return SpeakerDiagnosticAudio(samples: samples, sampleRate: sampleRate)
}

private func speakerDiagnosticNumber(_ value: Float) -> String {
    String(
        format: "%.6f",
        locale: Locale(identifier: "en_US_POSIX"),
        Double(value)
    )
}

private struct SpeakerDiagnosticDistribution {
    let count: Int
    let minimum: Float?
    let p25: Float?
    let median: Float?
    let p75: Float?
    let maximum: Float?
}

private func speakerDiagnosticQuantile(_ values: [Float], fraction: Double) -> Float? {
    guard !values.isEmpty else { return nil }
    let sorted = values.sorted()
    let position = Double(sorted.count - 1) * fraction
    let lowerIndex = Int(position.rounded(.down))
    let upperIndex = Int(position.rounded(.up))
    guard lowerIndex != upperIndex else { return sorted[lowerIndex] }
    let weight = Float(position - Double(lowerIndex))
    return sorted[lowerIndex] * (1 - weight) + sorted[upperIndex] * weight
}

private func speakerDiagnosticDistribution(_ values: [Float]) -> SpeakerDiagnosticDistribution {
    guard !values.isEmpty else {
        return SpeakerDiagnosticDistribution(
            count: 0,
            minimum: nil,
            p25: nil,
            median: nil,
            p75: nil,
            maximum: nil
        )
    }
    let sorted = values.sorted()
    return SpeakerDiagnosticDistribution(
        count: sorted.count,
        minimum: sorted.first,
        p25: speakerDiagnosticQuantile(sorted, fraction: 0.25),
        median: speakerDiagnosticQuantile(sorted, fraction: 0.50),
        p75: speakerDiagnosticQuantile(sorted, fraction: 0.75),
        maximum: sorted.last
    )
}

private func speakerDiagnosticValue(_ value: Float?) -> String {
    value.map(speakerDiagnosticNumber) ?? "na"
}

private func speakerDiagnosticMaximumAbsoluteDifference(
    _ left: [Float],
    _ right: [Float]
) -> Float {
    guard left.count == right.count else { return .infinity }
    return zip(left, right).reduce(Float.zero) { result, pair in
        max(result, abs(pair.0 - pair.1))
    }
}

private func speakerDiagnosticMaximumAbsoluteDifference(
    _ left: [[Float]],
    _ right: [[Float]]
) -> Float {
    guard left.count == right.count,
          zip(left, right).allSatisfy({ $0.0.count == $0.1.count }) else {
        return .infinity
    }
    return zip(left, right).reduce(Float.zero) { result, pair in
        max(
            result,
            speakerDiagnosticMaximumAbsoluteDifference(pair.0, pair.1)
        )
    }
}

private func speakerDiagnosticReferenceRoot(at inputPath: String) throws -> URL {
    let inputURL = URL(fileURLWithPath: inputPath)
    var isDirectory: ObjCBool = false
    guard FileManager.default.fileExists(
        atPath: inputURL.path,
        isDirectory: &isDirectory
    ) else {
        throw SpeakerIdentificationFailure.diagnosisInputMissing
    }
    return isDirectory.boolValue ? inputURL : inputURL.deletingLastPathComponent()
}

private func speakerDiagnosticDisplayPath(
    _ file: URL,
    inputPaths: [String]
) -> String {
    let filePath = file.standardizedFileURL.path
    for inputPath in inputPaths {
        let inputURL = URL(fileURLWithPath: inputPath).standardizedFileURL
        var isDirectory: ObjCBool = false
        let root = FileManager.default.fileExists(
            atPath: inputURL.path,
            isDirectory: &isDirectory
        ) && isDirectory.boolValue
            ? inputURL
            : inputURL.deletingLastPathComponent()
        let rootPath = root.standardizedFileURL.path
        if filePath.hasPrefix(rootPath + "/") {
            return String(filePath.dropFirst(rootPath.count + 1))
        }
    }
    return file.lastPathComponent
}

private func speakerDiagnosticReferenceFileURL(
    _ referencePath: String,
    root: URL
) throws -> URL {
    guard !referencePath.isEmpty,
          !referencePath.hasPrefix("/"),
          !referencePath.split(separator: "/").contains("..") else {
        throw SpeakerIdentificationFailure.diagnosisInputRead(
            "診断参照のファイルパスが不正です"
        )
    }
    let rootPath = root.standardizedFileURL.path
    let fileURL = root.appendingPathComponent(referencePath).standardizedFileURL
    guard fileURL.path.hasPrefix(rootPath + "/") else {
        throw SpeakerIdentificationFailure.diagnosisInputRead(
            "診断参照のファイルパスが入力範囲外です"
        )
    }
    return fileURL
}

private func compareSpeakerDiagnosticReference(
    at referenceURL: URL,
    inputPath: String,
    predictor: CoreMLSpeakerEmbeddingPredictor
) throws {
    let data = try Data(contentsOf: referenceURL)
    let reference = try JSONDecoder().decode(
        SpeakerDiagnosticReference.self,
        from: data
    )
    guard reference.schemaVersion == 1,
          !reference.files.isEmpty else {
        throw SpeakerIdentificationFailure.diagnosisInputRead(
            "診断参照のスキーマまたはファイル一覧が不正です"
        )
    }
    let root = try speakerDiagnosticReferenceRoot(at: inputPath)
    var resampleErrors: [Float] = []
    var fbankErrors: [Float] = []
    var rawOutputErrors: [Float] = []
    var normalizedErrors: [Float] = []
    var normalizedCosines: [Float] = []
    for item in reference.files {
        let fileURL = try speakerDiagnosticReferenceFileURL(item.path, root: root)
        let audio = try readSpeakerDiagnosticAudio(at: fileURL)
        guard audio.sampleRate == item.sourceSampleRate else {
            throw SpeakerIdentificationFailure.diagnosisInputRead(
                "診断参照と WAV のサンプルレートが一致しません: \(item.path)"
            )
        }
        let sampleCounts = try speakerWindowSampleCounts(for: audio.sampleRate)
        guard item.sourceStartSample >= 0,
              item.sourceStartSample <= audio.samples.count,
              sampleCounts.window <= audio.samples.count - item.sourceStartSample else {
            throw SpeakerIdentificationFailure.diagnosisInputRead(
                "診断参照の窓位置が WAV の範囲外です: \(item.path)"
            )
        }
        let sourceWindow = Array(
            audio.samples[item.sourceStartSample..<(item.sourceStartSample + sampleCounts.window)]
        )
        let resampled = try speakerResample(sourceWindow, from: audio.sampleRate)
        let features = try WeSpeakerFbank.features(for: resampled)
        let rawEmbedding = try predictor.predictRaw(features: features)
        let normalizedEmbedding = try normalizedEmbedding(rawEmbedding)
        let resampleError = speakerDiagnosticMaximumAbsoluteDifference(
            resampled,
            item.resampled
        )
        let fbankError = speakerDiagnosticMaximumAbsoluteDifference(
            features,
            item.features
        )
        let rawOutputError = speakerDiagnosticMaximumAbsoluteDifference(
            rawEmbedding,
            item.rawEmbedding
        )
        let normalizedError = speakerDiagnosticMaximumAbsoluteDifference(
            normalizedEmbedding,
            item.normalizedEmbedding
        )
        let normalizedCosine = cosineSimilarity(
            normalizedEmbedding,
            item.normalizedEmbedding
        )
        resampleErrors.append(resampleError)
        fbankErrors.append(fbankError)
        rawOutputErrors.append(rawOutputError)
        normalizedErrors.append(normalizedError)
        normalizedCosines.append(normalizedCosine)
        emitSpeakerDiagnostic(
            "speaker-diagnose-reference file=\(item.path) "
                + "source-start=\(item.sourceStartSample) "
                + "resample-max-abs=\(speakerDiagnosticNumber(resampleError)) "
                + "fbank-max-abs=\(speakerDiagnosticNumber(fbankError)) "
                + "coreml-raw-max-abs=\(speakerDiagnosticNumber(rawOutputError)) "
                + "normalized-cosine=\(speakerDiagnosticNumber(normalizedCosine)) "
                + "normalized-max-abs=\(speakerDiagnosticNumber(normalizedError))"
        )
    }
    let cosineDistribution = speakerDiagnosticDistribution(normalizedCosines)
    emitSpeakerDiagnostic(
        "speaker-diagnose-reference-summary count=\(reference.files.count) "
            + "input-shape=1x200x80 output-shape=1x256 "
            + "resample-max-abs=\(speakerDiagnosticNumber(resampleErrors.max() ?? .infinity)) "
            + "fbank-max-abs=\(speakerDiagnosticNumber(fbankErrors.max() ?? .infinity)) "
            + "coreml-raw-max-abs=\(speakerDiagnosticNumber(rawOutputErrors.max() ?? .infinity)) "
            + "normalized-cosine-min=\(speakerDiagnosticNumber(cosineDistribution.minimum ?? -.infinity)) "
            + "normalized-cosine-median=\(speakerDiagnosticNumber(cosineDistribution.median ?? -.infinity)) "
            + "normalized-max-abs=\(speakerDiagnosticNumber(normalizedErrors.max() ?? .infinity))"
    )
}

private func speakerDiagnosticCandidateText(
    _ scores: [(id: String, score: Float)]
) -> String {
    guard !scores.isEmpty else { return "none" }
    return scores.map {
        "\($0.id):\(speakerDiagnosticNumber($0.score))"
    }.joined(separator: ",")
}

private struct SpeakerDiagnosticPolicySummary {
    var counts: [SpeakerIdentificationStatusValue: Int] = [:]
    var identifiedSegments: [(id: String, embedding: [Float])] = []

    mutating func append(_ outcome: SpeakerDiagnosisIdentification) {
        let status = outcome.diagnostic.status
        counts[status, default: 0] += 1
        guard status == .identified,
              let id = outcome.speakerID,
              let embedding = outcome.segmentEmbedding else {
            return
        }
        identifiedSegments.append((id: id, embedding: embedding))
    }
}

private func speakerDiagnosticSegmentSimilarities(
    _ segments: [(id: String, embedding: [Float])],
    sameID: Bool
) -> [Float] {
    guard segments.count > 1 else { return [] }
    var similarities: [Float] = []
    similarities.reserveCapacity(segments.count * (segments.count - 1) / 2)
    for leftIndex in 0..<(segments.count - 1) {
        for rightIndex in (leftIndex + 1)..<segments.count {
            let idsMatch = segments[leftIndex].id == segments[rightIndex].id
            guard idsMatch == sameID else { continue }
            similarities.append(
                cosineSimilarity(
                    segments[leftIndex].embedding,
                    segments[rightIndex].embedding
                )
            )
        }
    }
    return similarities
}

private func emitSpeakerDiagnosticPolicySummary(
    label: String,
    summary: SpeakerDiagnosticPolicySummary,
    ledger: SpeakerLedger
) {
    let sameID = speakerDiagnosticDistribution(
        speakerDiagnosticSegmentSimilarities(summary.identifiedSegments, sameID: true)
    )
    let differentID = speakerDiagnosticDistribution(
        speakerDiagnosticSegmentSimilarities(summary.identifiedSegments, sameID: false)
    )
    emitSpeakerDiagnostic(
        "speaker-diagnose-\(label) "
            + "identified=\(summary.counts[.identified, default: 0]) "
            + "unknown=\(summary.counts[.unknown, default: 0]) "
            + "mixed=\(summary.counts[.mixed, default: 0]) "
            + "unavailable=\(summary.counts[.unavailable, default: 0]) "
            + "registered-speakers=\(ledger.activeProfileCount)"
    )
    emitSpeakerDiagnostic(
        "speaker-diagnose-\(label)-same-id "
            + "count=\(sameID.count) "
            + "min=\(speakerDiagnosticValue(sameID.minimum)) "
            + "p25=\(speakerDiagnosticValue(sameID.p25)) "
            + "median=\(speakerDiagnosticValue(sameID.median)) "
            + "p75=\(speakerDiagnosticValue(sameID.p75)) "
            + "max=\(speakerDiagnosticValue(sameID.maximum))"
    )
    emitSpeakerDiagnostic(
        "speaker-diagnose-\(label)-different-id "
            + "count=\(differentID.count) "
            + "min=\(speakerDiagnosticValue(differentID.minimum)) "
            + "p25=\(speakerDiagnosticValue(differentID.p25)) "
            + "median=\(speakerDiagnosticValue(differentID.median)) "
            + "p75=\(speakerDiagnosticValue(differentID.p75)) "
            + "max=\(speakerDiagnosticValue(differentID.maximum))"
    )
}

private func cleanupSpeakerDiagnosisArtifacts(at paths: [URL]) -> String? {
    var errors: [String] = []
    for path in paths {
        guard FileManager.default.fileExists(atPath: path.path) else { continue }
        do {
            try FileManager.default.removeItem(at: path)
        } catch {
            errors.append("\(path.lastPathComponent):\(error.localizedDescription)")
        }
    }
    return errors.isEmpty ? nil : errors.joined(separator: ",")
}

struct SpeakerDiagnosisThresholdParseError: Error {
    let message: String
}

func parseSpeakerDiagnosisThresholdOverride(
    arguments: [String]
) -> Result<
    (override: SpeakerDiagnosisThresholdOverride, remainingArguments: [String]),
    SpeakerDiagnosisThresholdParseError
> {
    var knownSimilarityThreshold: Float?
    var newSpeakerSimilarityThreshold: Float?
    var windowConsistencyThreshold: Float?
    var remainingArguments: [String] = []
    var index = 0
    while index < arguments.count {
        switch arguments[index] {
        case "--speaker-known-threshold":
            guard knownSimilarityThreshold == nil,
                  index + 1 < arguments.count,
                  let value = Float(arguments[index + 1]),
                  value >= 0, value <= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-known-threshold は 0 以上 1 以下の数値を一度だけ指定してください"
                ))
            }
            knownSimilarityThreshold = value
            index += 2
        case "--speaker-new-threshold":
            guard newSpeakerSimilarityThreshold == nil,
                  index + 1 < arguments.count,
                  let value = Float(arguments[index + 1]),
                  value >= 0, value <= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-new-threshold は 0 以上 1 以下の数値を一度だけ指定してください"
                ))
            }
            newSpeakerSimilarityThreshold = value
            index += 2
        case "--speaker-window-threshold":
            guard windowConsistencyThreshold == nil,
                  index + 1 < arguments.count,
                  let value = Float(arguments[index + 1]),
                  value >= -1, value <= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-window-threshold は -1 以上 1 以下の数値を一度だけ指定してください"
                ))
            }
            windowConsistencyThreshold = value
            index += 2
        default:
            remainingArguments.append(arguments[index])
            index += 1
        }
    }
    let effectiveKnown = knownSimilarityThreshold ?? speakerKnownSimilarityThreshold
    let effectiveNew = newSpeakerSimilarityThreshold ?? speakerNewSpeakerSimilarityThreshold
    guard effectiveNew <= effectiveKnown else {
        return .failure(SpeakerDiagnosisThresholdParseError(
            message: "--speaker-new-threshold は --speaker-known-threshold 以下の値を指定してください"
        ))
    }
    return .success((
        override: SpeakerDiagnosisThresholdOverride(
            knownSimilarityThreshold: knownSimilarityThreshold,
            newSpeakerSimilarityThreshold: newSpeakerSimilarityThreshold,
            windowConsistencyThreshold: windowConsistencyThreshold
        ),
        remainingArguments: remainingArguments
    ))
}

func runSpeakerDiagnosisCommandIfRequested() -> Bool {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.contains("--speaker-diagnose") else { return false }

    let thresholdOverride: SpeakerDiagnosisThresholdOverride
    let optionArguments: [String]
    switch parseSpeakerDiagnosisThresholdOverride(arguments: arguments) {
    case .success(let parsed):
        thresholdOverride = parsed.override
        optionArguments = parsed.remainingArguments
    case .failure(let error):
        emitSpeakerDiagnostic("speaker-diagnose error=\(error.message)")
        exit(2)
    }

    var inputPaths: [String] = []
    var modelPath: String?
    var referencePath: String?
    var microphone = false
    let centroidAutoUpdateStatus = speakerCentroidAutoUpdateEnabled
        ? "enabled"
        : "disabled-until-labeled-calibration"
    var index = 0
    while index < optionArguments.count {
        switch optionArguments[index] {
        case "--speaker-diagnose":
            guard index + 1 < optionArguments.count,
                  !optionArguments[index + 1].isEmpty,
                  !optionArguments[index + 1].hasPrefix("--") else {
                emitSpeakerDiagnostic("speaker-diagnose error=--speaker-diagnose <path> を指定してください")
                exit(2)
            }
            inputPaths.append(optionArguments[index + 1])
            index += 2
        case "--speaker-diagnose-microphone":
            guard !microphone else {
                emitSpeakerDiagnostic("speaker-diagnose error=--speaker-diagnose-microphone は一度だけ指定してください")
                exit(2)
            }
            microphone = true
            index += 1
        case "--speaker-model":
            guard modelPath == nil,
                  index + 1 < optionArguments.count,
                  !optionArguments[index + 1].isEmpty,
                  !optionArguments[index + 1].hasPrefix("--") else {
                emitSpeakerDiagnostic("speaker-diagnose error=--speaker-model <path> を一度だけ指定してください")
                exit(2)
            }
            modelPath = optionArguments[index + 1]
            index += 2
        case "--speaker-reference":
            guard referencePath == nil,
                  index + 1 < optionArguments.count,
                  !optionArguments[index + 1].isEmpty,
                  !optionArguments[index + 1].hasPrefix("--") else {
                emitSpeakerDiagnostic("speaker-diagnose error=--speaker-reference <json> を一度だけ指定してください")
                exit(2)
            }
            referencePath = optionArguments[index + 1]
            index += 2
        default:
            emitSpeakerDiagnostic("speaker-diagnose error=未対応の引数です: \(optionArguments[index])")
            exit(2)
        }
    }
    guard !inputPaths.isEmpty else {
        emitSpeakerDiagnostic("speaker-diagnose error=入力パスがありません")
        exit(2)
    }
    guard let modelPath else {
        emitSpeakerDiagnostic("speaker-diagnose error=--speaker-model <path> が必要です")
        exit(2)
    }
    if referencePath != nil && inputPaths.count != 1 {
        emitSpeakerDiagnostic("speaker-diagnose error=--speaker-reference は入力パス1件で指定してください")
        exit(2)
    }
    let currentRule = SpeakerDecisionRule.dominantCluster.applying(
        thresholdOverride: thresholdOverride
    )

    let modelCacheDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "coosenpai-speaker-diagnose-model-\(UUID().uuidString)",
            isDirectory: true
        )
    let diagnosisLedgerDirectory = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "coosenpai-speaker-diagnose-ledger-\(UUID().uuidString)",
            isDirectory: true
        )
    let temporaryArtifacts = [modelCacheDirectory, diagnosisLedgerDirectory]
    do {
        let files = try inputPaths.flatMap {
            try speakerDiagnosticInputFiles(at: $0, microphone: microphone)
        }
        try FileManager.default.createDirectory(
            at: diagnosisLedgerDirectory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        let predictor = try CoreMLSpeakerEmbeddingPredictor(
            path: modelPath,
            cacheDirectory: modelCacheDirectory
        )
        if let referencePath {
            try compareSpeakerDiagnosticReference(
                at: URL(fileURLWithPath: referencePath),
                inputPath: inputPaths[0],
                predictor: predictor
            )
        }
        let legacyLedger = try SpeakerLedger(
            forDiagnosisAt: diagnosisLedgerDirectory
                .appendingPathComponent("legacy/registry.enc").path,
            modelPackageDigest: predictor.modelPackageDigest
        )
        let currentLedger = try SpeakerLedger(
            forDiagnosisAt: diagnosisLedgerDirectory
                .appendingPathComponent("current/registry.enc").path,
            modelPackageDigest: predictor.modelPackageDigest
        )
        var legacySummary = SpeakerDiagnosticPolicySummary()
        var currentSummary = SpeakerDiagnosticPolicySummary()
        var allPairwiseSimilarities: [Float] = []
        allPairwiseSimilarities.reserveCapacity(files.count * 10)
        var segmentDurations: [Float] = []
        segmentDurations.reserveCapacity(files.count)
        for (index, file) in files.enumerated() {
            let audio = try readSpeakerDiagnosticAudio(at: file)
            segmentDurations.append(Float(Double(audio.samples.count) / audio.sampleRate))
            let windows = try speakerEmbeddingWindows(
                from: audio.samples,
                sampleRate: audio.sampleRate,
                predictor: predictor
            )
            let legacy = try legacyLedger.identifyForDiagnosis(
                windows: windows,
                generation: index + 1,
                rule: .allPairwise
            )
            let current = try currentLedger.identifyForDiagnosis(
                windows: windows,
                generation: index + 1,
                rule: currentRule
            )
            allPairwiseSimilarities.append(contentsOf: legacy.diagnostic.pairwiseSimilarities)
            legacySummary.append(legacy)
            currentSummary.append(current)
            let pairDistribution = speakerDiagnosticDistribution(
                legacy.diagnostic.pairwiseSimilarities
            )
            let displayPath = speakerDiagnosticDisplayPath(
                file,
                inputPaths: inputPaths
            )
            emitSpeakerDiagnostic(
                "speaker-diagnose file=\(displayPath) "
                    + "duration-seconds=\(speakerDiagnosticNumber(segmentDurations[index])) "
                    + "windows=\(legacy.diagnostic.windowCount) "
                    + "pair-count=\(pairDistribution.count) "
                    + "pair-min=\(speakerDiagnosticValue(pairDistribution.minimum)) "
                    + "pair-median=\(speakerDiagnosticValue(pairDistribution.median)) "
                    + "pair-max=\(speakerDiagnosticValue(pairDistribution.maximum)) "
                    + "legacy-judgment=\(legacy.diagnostic.status.rawValue) "
                    + "legacy-speaker=\(legacy.speakerID ?? "none") "
                    + "legacy-candidates=\(speakerDiagnosticCandidateText(legacy.diagnostic.candidateScores)) "
                    + "current-judgment=\(current.diagnostic.status.rawValue) "
                    + "current-speaker=\(current.speakerID ?? "none") "
                    + "current-primary-cluster=\(current.diagnostic.primaryClusterCount) "
                    + "current-secondary-cluster=\(current.diagnostic.secondaryClusterCount) "
                    + "current-candidates=\(speakerDiagnosticCandidateText(current.diagnostic.candidateScores))"
            )
        }
        let pairDistribution = speakerDiagnosticDistribution(allPairwiseSimilarities)
        let durationDistribution = speakerDiagnosticDistribution(segmentDurations)
        emitSpeakerDiagnostic(
            "speaker-diagnose-window-pair-cosine "
                + "count=\(pairDistribution.count) "
                + "min=\(speakerDiagnosticValue(pairDistribution.minimum)) "
                + "p25=\(speakerDiagnosticValue(pairDistribution.p25)) "
                + "median=\(speakerDiagnosticValue(pairDistribution.median)) "
                + "p75=\(speakerDiagnosticValue(pairDistribution.p75)) "
                + "max=\(speakerDiagnosticValue(pairDistribution.maximum))"
        )
        emitSpeakerDiagnostic(
            "speaker-diagnose-segment-duration-seconds "
                + "count=\(durationDistribution.count) "
                + "min=\(speakerDiagnosticValue(durationDistribution.minimum)) "
                + "p25=\(speakerDiagnosticValue(durationDistribution.p25)) "
                + "median=\(speakerDiagnosticValue(durationDistribution.median)) "
                + "p75=\(speakerDiagnosticValue(durationDistribution.p75)) "
                + "max=\(speakerDiagnosticValue(durationDistribution.maximum))"
        )
        emitSpeakerDiagnostic(
            "speaker-diagnose-rules "
                + "legacy-window-threshold=\(speakerDiagnosticNumber(speakerLegacyWindowConsistencyThreshold)) "
                + "legacy-known-threshold=\(speakerDiagnosticNumber(SpeakerDecisionRule.allPairwise.knownSimilarityThreshold)) "
                + "legacy-new-threshold=\(speakerDiagnosticNumber(SpeakerDecisionRule.allPairwise.newSpeakerSimilarityThreshold)) "
                + "current-window-threshold=\(speakerDiagnosticNumber(currentRule.windowConsistencyThreshold)) "
                + "primary-cluster-fraction=\(speakerDiagnosticNumber(Float(speakerWindowPrimaryClusterFraction))) "
                + "secondary-cluster-fraction=\(speakerDiagnosticNumber(Float(speakerWindowSecondaryClusterFraction))) "
                + "known-threshold=\(speakerDiagnosticNumber(currentRule.knownSimilarityThreshold)) "
                + "new-threshold=\(speakerDiagnosticNumber(currentRule.newSpeakerSimilarityThreshold)) "
                + "override=\(thresholdOverride.hasOverride ? "yes" : "no") "
                + "centroid-update-threshold=\(speakerDiagnosticNumber(speakerCentroidUpdateSimilarityThreshold)) "
                + "centroid-update-margin=\(speakerDiagnosticNumber(speakerCentroidUpdateMarginThreshold)) "
                + "centroid-update-min-windows=\(speakerCentroidUpdateMinimumWindowCount) "
                + "centroid-auto-update=\(centroidAutoUpdateStatus) "
                + "matching=weighted-all-effective-windows"
        )
        emitSpeakerDiagnosticPolicySummary(
            label: "legacy",
            summary: legacySummary,
            ledger: legacyLedger
        )
        emitSpeakerDiagnosticPolicySummary(
            label: "current",
            summary: currentSummary,
            ledger: currentLedger
        )
        if let cleanupError = cleanupSpeakerDiagnosisArtifacts(at: temporaryArtifacts) {
            emitSpeakerDiagnostic("speaker-diagnose error=一時ファイルを削除できません: \(cleanupError)")
            exit(1)
        }
        return true
    } catch {
        let cleanupError = cleanupSpeakerDiagnosisArtifacts(at: temporaryArtifacts)
        let cleanupText = cleanupError.map { " cleanup-error=\($0)" } ?? ""
        emitSpeakerDiagnostic(
            "speaker-diagnose error=\(error.localizedDescription)\(cleanupText)"
        )
        exit(1)
    }
}
