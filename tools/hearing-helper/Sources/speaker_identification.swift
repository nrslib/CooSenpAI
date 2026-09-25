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
let speakerIdentificationDecisionVersion = "speaker-cosine-ledger-v8"
private let speakerPreviousDecisionVersions = ["speaker-cosine-ledger-v6", "speaker-cosine-ledger-v7"]

private let speakerIdentificationModelPackageFiles = [
    (path: "Manifest.json", sha256: "b8c0b0d83caef37e7fe6d4ad8006085c16e916145c1e705d6079eefdc1c78af7"),
    (path: "Data/com.apple.CoreML/model.mlmodel", sha256: "61155e50979e8aab98226f81ae24327428d70e390eb1733e7109d4fcf00f1962"),
    (path: "Data/com.apple.CoreML/weights/weight.bin", sha256: speakerIdentificationModelWeightSHA256),
]
private let speakerIdentificationCompiledModelCacheVersion = "coreml-compiled-v1"

private let speakerSampleRate = 16_000.0
// 窓長の唯一の定義。テストからも参照するため internal とする。
let speakerWindowSampleCount = 32_240
private let speakerEvidenceWindowSampleCount = 32_000
// 期間の証拠窓の間隔。期間は区間より短く、2 秒間隔では 4 秒未満の期間が
// 単一窓扱いで既知照合しかできなかったため、半分重なりの 1 秒間隔で数える。
let speakerPeriodEvidenceWindowSampleCount = 16_000
private let speakerWindowDurationSeconds = 2.015
// 窓のずらし幅。期間の境目を 0.25 秒の精度で取るための唯一の定義で、
// 本番 session と診断の両方が speakerWindowSampleCounts 経由で同じ値を使う。
let speakerWindowShiftSeconds = 0.25
private let speakerWindowFrameCount = 200
private let speakerFrameSampleCount = 400
let speakerFrameShiftSampleCount = 160
private let speakerMelBinCount = 80
// 窓の最低品質。境界検出と照合の窓は 0.5 秒の有声があれば使う。
// 旧値 24,180（1.511 秒）は登録に必要な証拠量を各窓へ誤って適用したもので、
// 発話の合間を含む普通の窓（実測で有声 0.94〜1.5 秒）をすべて捨てていた。
private let speakerMinimumSpeechSamples = 8_000
// 新規登録に必要な有声時間。期間・区間では窓が実際に覆った有声 10ms フレームの
// 和集合で数え、保留候補からの登録では観測ごとの和集合を合計する。
// 無音・短い相づちを登録しない契約側の下限であり、窓ごとの品質とは分ける。
private let speakerEnrollmentMinimumSpeechSamples = 24_180
private let speakerLegacyKnownSimilarityThreshold: Float = 0.65
private let speakerLegacySingleWindowSimilarityThreshold: Float = 0.75
private let speakerLegacyNewSpeakerSimilarityThreshold: Float = 0.45
// 旧規則（allPairwise）の次点差。旧規則は第8周回より前の比較条件を再現するため、
// 現行規則の 0.05 とは別に 0.10 で固定する。
private let speakerLegacyMarginThreshold: Float = 0.10
// 現行規則の既定値。2026-09-20 の第8周回で、期間の証拠窓を1秒間隔にしたうえで
// 実動画265区間（第6周回と同じ対象、正解ラベルなし）の総当たりから採用した。
// 単一窓の既知一致だけは証拠が1窓のため 0.35 の厳しい値を維持する。
// 根拠は docs/plans/speaker-id-2026-09-16.md §4 を参照。
let speakerKnownSimilarityThreshold: Float = 0.25
private let speakerSingleWindowSimilarityThreshold: Float = 0.35
private let speakerNewSpeakerSimilarityThreshold: Float = 0.15
private let speakerLegacyWindowConsistencyThreshold: Float = 0.75
private let speakerWindowConsistencyThreshold: Float = 0.10
// 期間の境目は、直前の非重複窓（1 窓分以上前に開始した最も近い窓）との cosine が
// このしきい値を深く下回る位置に置く。無音なしの交代では 2 秒窓の重なりで遷移が
// なだらかになり、隣接窓や 1 秒振り返りの比較では交代が落ちきらない。非重複の比較は
// 正解ラベル付きの連続交代 fixture で交代の中心が 0.15 以下（多くは 0.02 以下）、
// 単独話者の 10 秒は 0.44 以上となり、0.25 で分離できることを確認した。
// 根拠は docs/plans/speaker-id-2026-09-16.md を参照。
let speakerPeriodBoundaryThreshold: Float = 0.25
// 一つの交代の遷移帯（窓の長さ分に広がる）が複数の境目を生まないよう、
// 確定した境目から 1 窓 + 1 ずらし幅（2.265 秒）以内の候補は弱い方を捨てる。
private let speakerPeriodBoundarySuppressionSampleCount = speakerWindowSampleCount
    + Int(speakerWindowShiftSeconds * speakerSampleRate)
private let speakerWindowPrimaryClusterFraction = 0.60
private let speakerWindowSecondaryClusterFraction = 0.40
// 新規登録に必要な証拠窓の数。期間では半分重なりの 1 秒間隔、区間では 2 秒間隔で選ぶ。
// 診断では --speaker-enroll-windows で上書きできる。
private let speakerEnrollmentEvidenceWindowCount = 2
// 既知一致で要求する上位1位と2位の差。区間全体向けに決めた 0.10 は短い期間に
// 厳しすぎた（best が既知しきい値を超えた期間のすべてが margin 未達で不明に
// なっていた）ため、第8周回の実データ評価で 0.05 に緩和した。
private let speakerMarginThreshold: Float = 0.05
private let speakerCentroidUpdateSimilarityThreshold: Float = 0.55
private let speakerCentroidUpdateMarginThreshold: Float = 0.15
private let speakerCentroidUpdateMinimumWindowCount = 3
// 正解ラベル付き校正が完了するまで、永続 centroid の自動更新は行わない。
private let speakerCentroidAutoUpdateEnabled = false
private let speakerMaximumProfiles = 1_000
private let speakerMaximumRegistryBytes = 16 * 1024 * 1024
private let speakerMaximumPendingCandidates = 64
private let speakerMaximumLegacyPendingCandidates = 32
private let speakerMaximumPendingReferences = 16
private let speakerMaximumBackfillEvidence = 512
private let speakerMaximumPendingCorrections = 512
private let speakerMaximumInvalidatedGenerations = 64
// 現行の判定根拠は、直近1時間の独立発言を最大64件まで保持する。
private let speakerRecentCandidateLifetimeSeconds = 60.0 * 60.0
private let speakerBackfillEvidenceLifetimeSeconds = 24.0 * 60.0 * 60.0
// 既存の厳しい更新候補条件を出発点にした保守的な値。2話者fixtureでの
// 回帰確認と、実運用のFAR/FRR校正は区別する（計画書のv7追補を参照）。
private let speakerTrustedSampleSimilarity: Float = 0.55
private let speakerRecentDirectSimilarityThreshold: Float = 0.30
private let speakerTrustedSampleMargin: Float = 0.15
private let speakerRecentDirectMarginThreshold: Float = 0.10
private let speakerMaximumTrustedSamplesPerID = 3
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

struct SpeakerIdentificationPeriod {
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
    var decisionDetails: [SpeakerDecisionDetails] = []
}

struct SpeakerIdentificationResult {
    let segmentID: String
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
    let periods: [SpeakerIdentificationPeriod]
    let corrections: [SpeakerIdentificationCorrection]
}

struct SpeakerFeatureSpeechTimeline {
    let featureStartAudioTimeNanoseconds: UInt64

    func speechDurationNanoseconds(until speechEndAudioTimeNanoseconds: UInt64) -> UInt64 {
        speechEndAudioTimeNanoseconds > featureStartAudioTimeNanoseconds
            ? speechEndAudioTimeNanoseconds - featureStartAudioTimeNanoseconds
            : 0
    }
}

struct SpeakerIdentificationCorrection: Codable, Equatable {
    let segmentID: String
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
    let speakerID: String
    let registryID: String
    let modelPackageDigest: String
    var decisionDetails: SpeakerDecisionDetails? = nil
}

struct SpeakerCandidateScore: Codable, Equatable {
    let speakerId: String
    let score: Float
}

struct SpeakerSupportingSample: Codable, Equatable {
    let segmentId: String
    let startMs: UInt64
    let endMs: UInt64
    let anchorId: String
    let anchorScore: Float
    let score: Float
}

struct SpeakerRecentComparison: Codable, Equatable {
    let segmentId: String
    let startMs: UInt64
    let endMs: UInt64
    let score: Float
}

// 公開するのは実判定時の数値と根拠の参照だけ。声紋・PCMは含めない。
struct SpeakerDecisionDetails: Codable, Equatable {
    let startMs: UInt64
    let endMs: UInt64
    let decisionVersion: String
    let registryId: String?
    let modelPackageDigest: String
    let phase: String
    let status: SpeakerIdentificationStatusValue
    let reason: String
    let candidates: [SpeakerCandidateScore]
    let candidateCount: Int
    let knownThreshold: Float
    let marginThreshold: Float
    let evidenceWindowCount: Int
    let voicedFrameCount: Int
    let supportingSamples: [SpeakerSupportingSample]
    let supportThreshold: Float?
    var recentCandidateCount: Int? = nil
    var recentMatchCount: Int? = nil
    var recentBestScore: Float? = nil
    var recentComparisons: [SpeakerRecentComparison]? = nil

    var eventFields: [String: Any] {
        var fields: [String: Any] = [
            "startMs": startMs, "endMs": endMs, "decisionVersion": decisionVersion,
            "modelPackageDigest": modelPackageDigest, "phase": phase,
            "status": status.rawValue, "reason": reason,
            "candidates": candidates.map { ["speakerId": $0.speakerId, "score": $0.score] as [String: Any] },
            "candidateCount": candidateCount, "knownThreshold": knownThreshold,
            "marginThreshold": marginThreshold, "evidenceWindowCount": evidenceWindowCount,
            "voicedFrameCount": voicedFrameCount,
            "supportingSamples": supportingSamples.map {
                ["segmentId": $0.segmentId, "startMs": $0.startMs, "endMs": $0.endMs,
                 "anchorId": $0.anchorId, "anchorScore": $0.anchorScore, "score": $0.score] as [String: Any]
            },
        ]
        if let registryId { fields["registryId"] = registryId }
        if let supportThreshold { fields["supportThreshold"] = supportThreshold }
        if let recentCandidateCount { fields["recentCandidateCount"] = recentCandidateCount }
        if let recentMatchCount { fields["recentMatchCount"] = recentMatchCount }
        if let recentBestScore { fields["recentBestScore"] = recentBestScore }
        if let recentComparisons {
            fields["recentComparisons"] = recentComparisons.map {
                ["segmentId": $0.segmentId, "startMs": $0.startMs, "endMs": $0.endMs, "score": $0.score] as [String: Any]
            }
        }
        return fields
    }
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
    if !result.periods.isEmpty {
        fields["speakerSegments"] = result.periods.map { period -> [String: Any] in
            var entry: [String: Any] = [
                "startMs": period.audioStartMilliseconds,
                "endMs": period.audioEndMilliseconds,
                "status": period.status.rawValue,
            ]
            if period.status == .identified, let speakerID = period.speakerID {
                entry["speakerId"] = speakerID
            }
            if period.status == .identified, let registryID = period.registryID {
                entry["speakerRegistryId"] = registryID
            }
            if !period.decisionDetails.isEmpty {
                entry["decisionDetails"] = period.decisionDetails.map(\.eventFields)
            }
            return entry
        }
    }
    if !result.corrections.isEmpty {
        fields["speakerCorrections"] = result.corrections.map { correction -> [String: Any] in
            var fields: [String: Any] = [
                "segmentId": correction.segmentID,
                "audioStartMs": correction.audioStartMilliseconds,
                "audioEndMs": correction.audioEndMilliseconds,
                "speakerId": correction.speakerID,
                "speakerRegistryId": correction.registryID,
                "modelPackageDigest": correction.modelPackageDigest,
            ]
            if let details = correction.decisionDetails {
                fields["decisionDetails"] = details.eventFields
            }
            return fields
        }
    }
    return fields
}

enum SpeakerIdentificationStatusValue: String, Codable {
    case identified
    case unknown
    case mixed
    case unavailable
}

// 期間の判定がどの分岐で決まったかを表す。identify(analysis:) の分岐と 1 対 1 に対応する。
enum SpeakerIdentificationDecisionReason: String, CaseIterable {
    case matchedKnown = "matched-known"
    case matchedSamples = "matched-samples"
    case replayedConfirmed = "replayed-confirmed"
    case ambiguousRepresentatives = "ambiguous-representatives"
    case enrolledNew = "enrolled-new"
    case enrolledPending = "enrolled-pending"
    case pendingCandidate = "pending-candidate"
    case belowKnownAboveNew = "below-known-above-new"
    case singleWindow = "single-window"
    case mixedClusters = "mixed-clusters"
    case noEvidence = "no-evidence"
}

enum SpeakerIdentificationPreparationStatus: String {
    case preparing
    case ready
    case unavailable
}

struct SpeakerEmbeddingWindow {
    let embedding: [Float]
    let startSample: Int
    // 窓内の有声 10ms フレームの位置（窓先頭からのフレーム番号）。
    // 登録判定の有声時間は窓の重なりを除いた和集合で数える。窓の重みもここから導く。
    let voicedFrames: Set<Int>

    var weight: Float {
        Float(voicedFrames.count * speakerFrameShiftSampleCount)
            / Float(speakerWindowSampleCount)
    }
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
    let periodBoundaryThreshold: Float?
    let enrollWindowCount: Int?
    let marginThreshold: Float?

    var hasOverride: Bool {
        knownSimilarityThreshold != nil
            || newSpeakerSimilarityThreshold != nil
            || windowConsistencyThreshold != nil
            || periodBoundaryThreshold != nil
            || enrollWindowCount != nil
            || marginThreshold != nil
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
    let enrollWindowCount: Int
    let allowsSingleWindowEnrollment: Bool
    let marginThreshold: Float

    static let allPairwise = SpeakerDecisionRule(
        windowClustering: .allPairwise,
        windowConsistencyThreshold: speakerLegacyWindowConsistencyThreshold,
        knownSimilarityThreshold: speakerLegacyKnownSimilarityThreshold,
        singleWindowSimilarityThreshold: speakerLegacySingleWindowSimilarityThreshold,
        newSpeakerSimilarityThreshold: speakerLegacyNewSpeakerSimilarityThreshold,
        enrollWindowCount: speakerEnrollmentEvidenceWindowCount,
        allowsSingleWindowEnrollment: true,
        marginThreshold: speakerLegacyMarginThreshold
    )

    static let dominantCluster = SpeakerDecisionRule(
        windowClustering: .dominantCluster,
        windowConsistencyThreshold: speakerWindowConsistencyThreshold,
        knownSimilarityThreshold: speakerKnownSimilarityThreshold,
        singleWindowSimilarityThreshold: speakerSingleWindowSimilarityThreshold,
        newSpeakerSimilarityThreshold: speakerNewSpeakerSimilarityThreshold,
        enrollWindowCount: speakerEnrollmentEvidenceWindowCount,
        allowsSingleWindowEnrollment: false,
        marginThreshold: speakerMarginThreshold
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
            enrollWindowCount: thresholdOverride.enrollWindowCount
                ?? enrollWindowCount,
            allowsSingleWindowEnrollment: allowsSingleWindowEnrollment,
            marginThreshold: thresholdOverride.marginThreshold
                ?? marginThreshold
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

private func validSpeakerID(_ value: String) -> Bool {
    guard let uuid = UUID(uuidString: value) else { return false }
    return uuid.uuidString.lowercased() == value
}

private struct SpeakerProfile: Codable {
    let id: String
    let anchor: [Float]
    var centroid: [Float]
    var updateCount: UInt32
    var state: String
}

struct PendingSpeakerEvidenceReference: Codable, Hashable {
    let segmentID: String
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
}

private struct PendingSpeakerCandidate: Codable {
    let generation: Int
    let embedding: [Float]
    let voicedFrameCount: Int
    let observedAt: Date
    var references: [PendingSpeakerEvidenceReference]
    var confirmedSpeakerID: String?
}

// 登録用の短命候補とは別に、過去の unknown 期間を各期間自身の埋め込みと
// 判定条件で再照合するための最小証拠。生音声は保持しない。
private struct SpeakerBackfillEvidence: Codable, Equatable {
    let registryID: String
    let modelPackageDigest: String
    let segmentID: String
    let audioStartMilliseconds: UInt64
    let audioEndMilliseconds: UInt64
    let embedding: [Float]
    let voicedFrameCount: Int
    let evidenceWindowCount: Int
    let singleWindowSimilarityThreshold: Float
    let knownSimilarityThreshold: Float
    let marginThreshold: Float
    let observedAt: Date
}

private struct TrustedSpeakerSample: Codable, Equatable {
    let registryID: String
    let modelPackageDigest: String
    let anchorID: String
    let reference: PendingSpeakerEvidenceReference
    let embedding: [Float]
    let anchorScore: Float
    let observedAt: Date
}

private struct SpeakerRegistryDocument: Codable {
    let schemaVersion: Int
    let registryID: String
    let modelIdentifier: String
    let modelPackageDigest: String
    let preprocessingVersion: String
    var decisionVersion: String
    var profiles: [SpeakerProfile]
    var aliases: [String: String]
    var pendingCandidates: [PendingSpeakerCandidate]
    var backfillEvidence: [SpeakerBackfillEvidence]
    var pendingCorrections: [SpeakerIdentificationCorrection]
    var trustedSamples: [TrustedSpeakerSample] = []
    var revision: UInt64

    private enum CodingKeys: String, CodingKey {
        case schemaVersion
        case registryID
        case modelIdentifier
        case modelPackageDigest
        case preprocessingVersion
        case decisionVersion
        case profiles
        case aliases
        case pendingCandidates
        case backfillEvidence
        case pendingCorrections
        case trustedSamples
        case revision
    }

    init(
        schemaVersion: Int,
        registryID: String,
        modelIdentifier: String,
        modelPackageDigest: String,
        preprocessingVersion: String,
        decisionVersion: String,
        profiles: [SpeakerProfile],
        aliases: [String: String],
        pendingCandidates: [PendingSpeakerCandidate],
        backfillEvidence: [SpeakerBackfillEvidence],
        pendingCorrections: [SpeakerIdentificationCorrection],
        revision: UInt64
    ) {
        self.schemaVersion = schemaVersion
        self.registryID = registryID
        self.modelIdentifier = modelIdentifier
        self.modelPackageDigest = modelPackageDigest
        self.preprocessingVersion = preprocessingVersion
        self.decisionVersion = decisionVersion
        self.profiles = profiles
        self.aliases = aliases
        self.pendingCandidates = pendingCandidates
        self.backfillEvidence = backfillEvidence
        self.pendingCorrections = pendingCorrections
        self.revision = revision
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(Int.self, forKey: .schemaVersion)
        registryID = try container.decode(String.self, forKey: .registryID)
        modelIdentifier = try container.decode(String.self, forKey: .modelIdentifier)
        modelPackageDigest = try container.decode(String.self, forKey: .modelPackageDigest)
        preprocessingVersion = try container.decode(String.self, forKey: .preprocessingVersion)
        decisionVersion = try container.decode(String.self, forKey: .decisionVersion)
        profiles = try container.decode([SpeakerProfile].self, forKey: .profiles)
        aliases = try container.decode([String: String].self, forKey: .aliases)
        pendingCandidates = try container.decodeIfPresent(
            [PendingSpeakerCandidate].self,
            forKey: .pendingCandidates
        ) ?? []
        backfillEvidence = try container.decodeIfPresent(
            [SpeakerBackfillEvidence].self,
            forKey: .backfillEvidence
        ) ?? []
        pendingCorrections = try container.decodeIfPresent(
            [SpeakerIdentificationCorrection].self,
            forKey: .pendingCorrections
        ) ?? []
        trustedSamples = try container.decodeIfPresent([TrustedSpeakerSample].self, forKey: .trustedSamples) ?? []
        revision = try container.decode(UInt64.self, forKey: .revision)
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(schemaVersion, forKey: .schemaVersion)
        try container.encode(registryID, forKey: .registryID)
        try container.encode(modelIdentifier, forKey: .modelIdentifier)
        try container.encode(modelPackageDigest, forKey: .modelPackageDigest)
        try container.encode(preprocessingVersion, forKey: .preprocessingVersion)
        try container.encode(decisionVersion, forKey: .decisionVersion)
        try container.encode(profiles, forKey: .profiles)
        try container.encode(aliases, forKey: .aliases)
        try container.encode(pendingCandidates, forKey: .pendingCandidates)
        try container.encode(backfillEvidence, forKey: .backfillEvidence)
        try container.encode(pendingCorrections, forKey: .pendingCorrections)
        try container.encode(trustedSamples, forKey: .trustedSamples)
        try container.encode(revision, forKey: .revision)
    }
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

}

protocol SpeakerKeyStore: AnyObject {
    func readExisting() throws -> SymmetricKey?
    func readOrCreate() throws -> SymmetricKey
    func delete() throws
}

// 話者管理 UI へ提示する読み取り専用の候補一覧。ID と別名対応だけを持ち、
// 声紋（anchor / centroid）・音声・埋め込みは含めない。
struct SpeakerManagementDirectory {
    let registryID: String
    let speakers: [String]
    let aliases: [String: String]
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
    private let clock: () -> Date
    private var document: SpeakerRegistryDocument?
    private var identificationFailure: SpeakerIdentificationFailure?

    convenience init(
        path: String,
        keyStore: SpeakerKeyStore? = nil,
        modelPackageDigest: String? = nil,
        beforePersistence: @escaping () -> Void = {},
        afterPersistenceBegan: @escaping () -> Void = {},
        afterCommitReserved: @escaping () -> Void = {},
        shouldFailAliasWrite: @escaping () -> Bool = { false },
        clock: @escaping () -> Date = { Date() }
    ) throws {
        try self.init(
            path: path,
            keyStore: keyStore,
            modelPackageDigest: modelPackageDigest,
            beforePersistence: beforePersistence,
            afterPersistenceBegan: afterPersistenceBegan,
            afterCommitReserved: afterCommitReserved,
            shouldFailAliasWrite: shouldFailAliasWrite,
            clock: clock,
            loadExistingDocument: true
        )
    }

    private init(
        path: String,
        keyStore: SpeakerKeyStore?,
        modelPackageDigest: String?,
        beforePersistence: @escaping () -> Void,
        afterPersistenceBegan: @escaping () -> Void,
        afterCommitReserved: @escaping () -> Void,
        shouldFailAliasWrite: @escaping () -> Bool,
        clock: @escaping () -> Date,
        loadExistingDocument: Bool
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
        self.clock = clock
        if Self.fileExists(at: self.lockPath) {
            try Self.removeStaleLockIfNeeded(at: self.lockPath)
            guard !Self.fileExists(at: self.lockPath) else {
                throw SpeakerIdentificationFailure.ledgerLocked
            }
        }
        if loadExistingDocument {
            if Self.fileExists(at: self.path) {
                try load()
            } else if try self.keyStore.readExisting() != nil {
                throw SpeakerIdentificationFailure.ledgerMissing
            }
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

    func noEvidencePeriod(start: UInt64, end: UInt64) throws -> SpeakerIdentificationPeriod {
        guard let digest = expectedModelPackageDigest else { throw SpeakerIdentificationFailure.modelPackageMismatch }
        return SpeakerIdentificationPeriod(
            audioStartMilliseconds: start, audioEndMilliseconds: end,
            speakerID: nil, registryID: nil, status: .unknown,
            decisionDetails: [SpeakerDecisionDetails(
                startMs: start, endMs: end, decisionVersion: speakerIdentificationDecisionVersion,
                registryId: document?.registryID, modelPackageDigest: digest,
                phase: "initial", status: .unknown, reason: "no-evidence",
                candidates: [], candidateCount: 0, knownThreshold: speakerKnownSimilarityThreshold,
                marginThreshold: speakerMarginThreshold, evidenceWindowCount: 0, voicedFrameCount: 0,
                supportingSamples: [], supportThreshold: nil
            )]
        )
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
        let voicedFrames: Set<Int>
        let windowConsistencyThreshold: Float
        let knownSimilarityThreshold: Float
        let singleWindowSimilarityThreshold: Float
        let newSpeakerSimilarityThreshold: Float
        let allowsSingleWindowEnrollment: Bool
        let marginThreshold: Float
    }

    private func analyze(
        windows: [SpeakerEmbeddingWindow],
        rule: SpeakerDecisionRule,
        evidenceStrideSamples: Int,
        compareProfiles: Bool
    ) throws -> SpeakerDecisionAnalysis? {
        guard !windows.isEmpty else { return nil }
        guard windows.allSatisfy({ $0.embedding.count == 256 }) else {
            throw SpeakerIdentificationFailure.invalidEmbedding
        }
        let allEvidenceWindows = nonOverlappingWindows(
            windows,
            strideSamples: evidenceStrideSamples
        )
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
        let activeProfiles = compareProfiles
            ? document?.profiles.filter { $0.state == "active" } ?? []
            : []
        let candidateScores: [(String, Float, Float)] = compareProfiles
            ? activeProfiles.compactMap { profile -> (String, Float, Float)? in
                let anchor = cosineSimilarity(segmentEmbedding, profile.anchor)
                return (canonicalID(profile.id), anchor, anchor)
            }
            : []
        let rankedCandidates = rankCandidates(candidateScores)
        let knownMaximum: Float = compareProfiles
            ? activeProfiles
                .map { profile in cosineSimilarity(segmentEmbedding, profile.anchor) }
                .max() ?? -1
            : -1
        // 登録は期間・区間全体の有声時間で判定する。重なった窓の有声フレームを
        // 二重に数えないよう、窓ごとの有声フレームの位置の和集合で数える。
        var voicedFrameSet = Set<Int>()
        for window in windows {
            let frameOffset = window.startSample / speakerFrameShiftSampleCount
            voicedFrameSet.formUnion(window.voicedFrames.map { $0 + frameOffset })
        }
        let enrollmentSpeechSamples = voicedFrameSet.count * speakerFrameShiftSampleCount
        let canEnroll = allEvidenceWindows.count >= rule.enrollWindowCount
            && !mixed
            && (!compareProfiles || knownMaximum <= rule.newSpeakerSimilarityThreshold)
            && enrollmentSpeechSamples >= speakerEnrollmentMinimumSpeechSamples
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
            voicedFrames: voicedFrameSet,
            windowConsistencyThreshold: windowConsistencyThreshold,
            knownSimilarityThreshold: rule.knownSimilarityThreshold,
            singleWindowSimilarityThreshold: rule.singleWindowSimilarityThreshold,
            newSpeakerSimilarityThreshold: rule.newSpeakerSimilarityThreshold,
            allowsSingleWindowEnrollment: rule.allowsSingleWindowEnrollment,
            marginThreshold: rule.marginThreshold
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
               || first.score - analysis.rankedCandidates[1].score
                   >= analysis.marginThreshold {
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
        guard let analysis = try analyze(
            windows: windows,
            rule: rule,
            evidenceStrideSamples: speakerEvidenceWindowSampleCount,
            compareProfiles: true
        ) else {
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
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil,
        evidenceReference: PendingSpeakerEvidenceReference? = nil
    ) throws -> (String?, String?, SpeakerIdentificationStatusValue) {
        if let identificationFailure {
            throw identificationFailure
        }
        guard let analysis = try analyze(
            windows: windows,
            rule: .dominantCluster,
            evidenceStrideSamples: speakerEvidenceWindowSampleCount,
            compareProfiles: false
        ) else {
            return (nil, nil, .unknown)
        }
        let decision = try identify(
            analysis: analysis,
            generation: generation,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit,
            evidenceReference: evidenceReference
        )
        return (decision.speakerID, decision.registryID, decision.status)
    }

    // 区間内の窓を時系列で見て、類似が落ちる位置で期間に分け、期間ごとに照合する。
    // 期間に分けられなかった部分（単一期間の混在判定）だけが mixed を維持する。
    func identifyPeriods(
        windows: [SpeakerEmbeddingWindow],
        boundaryThreshold: Float = speakerPeriodBoundaryThreshold,
        rule: SpeakerDecisionRule = .dominantCluster,
        generation: Int = 0,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil,
        segmentID: String? = nil,
        segmentStartMilliseconds: UInt64? = nil
    ) throws -> SpeakerPeriodIdentification {
        if let identificationFailure {
            throw identificationFailure
        }
        let ranges = speakerWindowPeriodRanges(windows, boundaryThreshold: boundaryThreshold)
        var periods: [SpeakerIdentifiedWindowPeriod] = []
        var corrections = replayablePendingCorrections()
        for range in ranges {
            // 境界をまたぐ窓（2 秒窓が両側の話者の音声を含む）は期間の照合と
            // 登録埋め込みに混ぜない。
            let containedWindows = range.windows.filter {
                $0.startSample >= range.startSample
                    && $0.startSample + speakerWindowSampleCount <= range.endSample
            }
            let decision: SpeakerIdentificationDecision
            let segmentEmbedding: [Float]?
            let evidenceWindowCount: Int
            let candidateScores: [(id: String, score: Float)]
            let canEnroll: Bool
            let voicedFrameCount: Int
            let evidenceReference = pendingEvidenceReference(
                segmentID: segmentID,
                segmentStartMilliseconds: segmentStartMilliseconds,
                startSample: range.startSample,
                endSample: range.endSample
            )
            if let analysis = try analyze(
                windows: containedWindows,
                rule: rule,
                evidenceStrideSamples: speakerPeriodEvidenceWindowSampleCount,
                compareProfiles: false
            ) {
                decision = try identify(
                    analysis: analysis,
                    generation: generation,
                    canCommit: canCommit,
                    authorizeCommit: authorizeCommit,
                    evidenceReference: evidenceReference
                )
                corrections.append(contentsOf: decision.corrections)
                segmentEmbedding = analysis.segmentEmbedding
                evidenceWindowCount = analysis.allEvidenceWindows.count
                candidateScores = decision.candidateScores.map { (id: $0.speakerId, score: $0.score) }
                canEnroll = analysis.canEnroll
                voicedFrameCount = analysis.voicedFrames.count
            } else {
                decision = SpeakerIdentificationDecision(
                    speakerID: nil,
                    registryID: nil,
                    status: .unknown,
                    reason: .noEvidence,
                    corrections: []
                )
                segmentEmbedding = nil
                evidenceWindowCount = 0
                candidateScores = []
                canEnroll = false
                voicedFrameCount = 0
            }
            let details: [SpeakerDecisionDetails]
            if let evidenceReference, let digest = expectedModelPackageDigest {
                details = [SpeakerDecisionDetails(
                    startMs: evidenceReference.audioStartMilliseconds, endMs: evidenceReference.audioEndMilliseconds,
                    decisionVersion: speakerIdentificationDecisionVersion, registryId: document?.registryID,
                    modelPackageDigest: digest, phase: "initial-recent", status: decision.status, reason: decision.reason.rawValue,
                    candidates: candidateScores.prefix(3).map { SpeakerCandidateScore(speakerId: $0.id, score: $0.score) },
                    candidateCount: candidateScores.count,
                    knownThreshold: speakerRecentDirectSimilarityThreshold,
                    // initial-recent は過去profileの順位を使わず、直近実観測の直接照合だけを使う。
                    marginThreshold: speakerRecentDirectMarginThreshold, evidenceWindowCount: evidenceWindowCount,
                    voicedFrameCount: voicedFrameCount, supportingSamples: decision.supportingSamples,
                    supportThreshold: decision.supportingSamples.count == 1
                        ? speakerRecentDirectSimilarityThreshold
                        : speakerTrustedSampleSimilarity,
                    recentCandidateCount: decision.recentCandidateCount,
                    recentMatchCount: decision.recentMatchCount,
                    recentBestScore: decision.recentBestScore,
                    recentComparisons: decision.recentComparisons
                )]
            } else { details = [] }
            periods.append(
                SpeakerIdentifiedWindowPeriod(
                    startSample: range.startSample,
                    endSample: range.endSample,
                    speakerID: decision.speakerID,
                    registryID: decision.registryID,
                    status: decision.status,
                    segmentEmbedding: decision.status == .identified ? segmentEmbedding : nil,
                    decisionReason: decision.reason,
                    windowCount: range.windows.count,
                    evidenceWindowCount: evidenceWindowCount,
                    candidateScores: candidateScores,
                    canEnroll: canEnroll,
                    decisionDetails: details
                )
            )
        }
        var merged: [SpeakerIdentifiedWindowPeriod] = []
        for period in periods {
            if let previous = merged.last,
               previous.status == .identified,
               period.status == .identified,
               previous.speakerID == period.speakerID {
                merged[merged.count - 1] = SpeakerIdentifiedWindowPeriod(
                    startSample: previous.startSample,
                    endSample: period.endSample,
                    speakerID: previous.speakerID,
                    registryID: previous.registryID,
                    status: previous.status,
                    segmentEmbedding: previous.segmentEmbedding,
                    decisionReason: previous.decisionReason,
                    windowCount: previous.windowCount + period.windowCount,
                    evidenceWindowCount: previous.evidenceWindowCount
                        + period.evidenceWindowCount,
                    candidateScores: previous.candidateScores,
                    canEnroll: previous.canEnroll,
                    decisionDetails: previous.decisionDetails + period.decisionDetails
                )
            } else {
                merged.append(period)
            }
        }
        guard merged.count > 1 else {
            let only = merged.first
            return SpeakerPeriodIdentification(
                periods: merged,
                speakerID: only?.speakerID,
                registryID: only?.registryID,
                status: only?.status ?? .unknown,
                corrections: uniqueCorrections(corrections)
            )
        }
        // 複数期間を持つ区間には区間全体の ID を付けない。詳細は期間の列が正本として持つ。
        // 同一 ID の期間は直前のマージで一つにまとまるため、複数期間の時点で
        // 「全期間が同一 ID で確定」にはなり得ない。
        return SpeakerPeriodIdentification(
            periods: merged,
            speakerID: nil,
            registryID: nil,
            status: .unknown,
            corrections: uniqueCorrections(corrections)
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
        guard let analysis = try analyze(
            windows: windows,
            rule: rule,
            evidenceStrideSamples: speakerEvidenceWindowSampleCount,
            compareProfiles: rule.windowClustering == .allPairwise
        ) else {
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
        let result = try rule.windowClustering == .allPairwise
            ? identifyLegacyAnchorBaseline(analysis: analysis, generation: generation)
            : identify(analysis: analysis, generation: generation)
        return SpeakerDiagnosisIdentification(
            diagnostic: SpeakerIdentificationDiagnostic(
                windowCount: analysis.allEvidenceWindows.count,
                pairwiseSimilarities: analysis.pairwiseSimilarities,
                candidateScores: rule.windowClustering == .allPairwise
                    ? analysis.rankedCandidates.map { (id: $0.id, score: $0.score) }
                    : result.candidateScores.map { (id: $0.speakerId, score: $0.score) },
                primaryClusterCount: analysis.primaryClusterCount,
                secondaryClusterCount: analysis.secondaryClusterCount,
                status: result.status
            ),
            speakerID: result.speakerID,
            registryID: result.registryID,
            segmentEmbedding: analysis.segmentEmbedding
        )
    }

    private func identifyLegacyAnchorBaseline(
        analysis: SpeakerDecisionAnalysis,
        generation: Int,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil,
        evidenceReference: PendingSpeakerEvidenceReference? = nil
    ) throws -> SpeakerIdentificationDecision {
        // これは offline の allPairwise 診断専用。現行の recent-only session は
        // identifyRecentCandidate() だけを通り、保存済み代表から訂正しない。
        if analysis.mixed {
            return SpeakerIdentificationDecision(
                speakerID: nil,
                registryID: nil,
                status: .mixed,
                reason: .mixedClusters,
                corrections: []
            )
        }
        let threshold = analysis.evidenceWindows.count == 1
            ? analysis.singleWindowSimilarityThreshold
            : analysis.knownSimilarityThreshold
        if let first = analysis.rankedCandidates.first,
           first.score >= threshold,
           (analysis.rankedCandidates.count == 1
               || first.score - analysis.rankedCandidates[1].score
                   >= analysis.marginThreshold) {
            let corrections = try backfillStoredEvidenceForLegacyDiagnosis(
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            return SpeakerIdentificationDecision(
                speakerID: first.id,
                registryID: document?.registryID,
                status: .identified,
                reason: .matchedKnown,
                corrections: corrections
            )
        }
        if analysis.canEnroll {
            let id = try registerLegacyProfile(
                embedding: analysis.segmentEmbedding,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            let corrections = try backfillStoredEvidenceForLegacyDiagnosis(
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            return SpeakerIdentificationDecision(
                speakerID: id,
                registryID: document?.registryID,
                status: .identified,
                reason: .enrolledNew,
                corrections: corrections
            )
        }
        guard analysis.evidenceWindows.count > 1 || analysis.allowsSingleWindowEnrollment else {
            try rememberBackfillEvidence(
                from: analysis,
                reference: evidenceReference,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            return SpeakerIdentificationDecision(
                speakerID: nil,
                registryID: nil,
                status: .unknown,
                reason: .singleWindow,
                corrections: []
            )
        }
        guard analysis.knownMaximum <= analysis.newSpeakerSimilarityThreshold else {
            try rememberBackfillEvidence(
                from: analysis,
                reference: evidenceReference,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
            return SpeakerIdentificationDecision(
                speakerID: nil,
                registryID: nil,
                status: .unknown,
                reason: .belowKnownAboveNew,
                corrections: []
            )
        }
        let pending = try enrollOrRememberLegacyCandidate(
            from: [
                SpeakerEmbeddingWindow(
                    embedding: analysis.segmentEmbedding,
                    startSample: 0,
                    voicedFrames: analysis.voicedFrames
                ),
            ],
            generation: generation,
            consistencyThreshold: analysis.windowConsistencyThreshold,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit,
            evidenceReference: evidenceReference
        )
        if pending.2 == .unknown {
            try rememberBackfillEvidence(
                from: analysis,
                reference: evidenceReference,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit
            )
        }
        return SpeakerIdentificationDecision(
            speakerID: pending.0,
            registryID: pending.1,
            status: pending.2,
            reason: pending.2 == .identified ? .enrolledPending : .pendingCandidate,
            corrections: pending.3
        )
    }

    private func identify(
        analysis: SpeakerDecisionAnalysis,
        generation: Int,
        canCommit: () -> Bool = { true },
        authorizeCommit: SpeakerLedgerCommitAuthorizer? = nil,
        evidenceReference: PendingSpeakerEvidenceReference? = nil
    ) throws -> SpeakerIdentificationDecision {
        if let reference = evidenceReference,
           let previous = document?.pendingCandidates.first(where: { $0.references == [reference] }),
           clock().timeIntervalSince(previous.observedAt) >= 0,
           clock().timeIntervalSince(previous.observedAt) <= speakerRecentCandidateLifetimeSeconds,
           let confirmedID = previous.confirmedSpeakerID,
           let confirmed = profile(for: confirmedID) {
            guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
            return SpeakerIdentificationDecision(
                speakerID: confirmed.id, registryID: document?.registryID, status: .identified,
                reason: .replayedConfirmed, corrections: [], recentCandidateCount: 0, recentMatchCount: 0
            )
        }
        if analysis.mixed {
            return SpeakerIdentificationDecision(
                speakerID: nil,
                registryID: nil,
                status: .mixed,
                reason: .mixedClusters,
                corrections: []
            )
        }
        let decision = try identifyRecentCandidate(
            analysis: analysis, generation: generation,
            canCommit: canCommit, authorizeCommit: authorizeCommit,
            evidenceReference: evidenceReference
        )
        if decision.status == .unknown {
            try rememberBackfillEvidence(
                from: analysis, reference: evidenceReference,
                canCommit: canCommit, authorizeCommit: authorizeCommit
            )
        }
        return decision
    }

    func discardPendingCandidates(for generation: Int) {
        guard var document else { return }
        // 登録済みの観測は直近照合の根拠なので、未確定候補だけを世代破棄する。
        let retained = document.pendingCandidates.filter {
            $0.generation != generation || $0.confirmedSpeakerID != nil
        }
        guard retained.count != document.pendingCandidates.count else { return }
        document.pendingCandidates = retained
        document.revision &+= 1
        do {
            try save(document)
            self.document = document
        } catch let error as SpeakerIdentificationFailure {
            identificationFailure = error
        } catch {
            identificationFailure = .ledgerWrite(error.localizedDescription)
        }
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
        let targetSamples = document.trustedSamples.filter { canonicalID($0.anchorID) == canonicalTarget }
        if targetSamples.isEmpty, let targetProfile = document.profiles.first(where: { $0.id == canonicalTarget }) {
            document.trustedSamples = document.trustedSamples.map { sample in
                guard sample.anchorID == source else { return sample }
                return TrustedSpeakerSample(
                    registryID: sample.registryID, modelPackageDigest: sample.modelPackageDigest,
                    anchorID: canonicalTarget, reference: sample.reference, embedding: sample.embedding,
                    anchorScore: cosineSimilarity(sample.embedding, targetProfile.anchor), observedAt: sample.observedAt
                )
            }
        } else {
            document.trustedSamples.removeAll { $0.anchorID == source }
        }
        if let sourceIndex = document.profiles.firstIndex(where: { $0.id == source }) {
            document.profiles[sourceIndex].updateCount = 0
        }
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
        document.trustedSamples.removeAll { $0.anchorID == id }
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
        document.trustedSamples.removeAll { $0.anchorID == id }
        document.aliases = document.aliases.filter { $0.key != id && $0.value != id }
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    func deleteAll() throws {
        guard var document else {
            try removeAliasIndex()
            return
        }
        document.profiles.removeAll()
        document.aliases.removeAll()
        document.pendingCandidates.removeAll()
        document.backfillEvidence.removeAll()
        document.pendingCorrections.removeAll()
        document.trustedSamples.removeAll()
        document.revision &+= 1
        try save(document)
        self.document = document
    }

    private func load() throws {
        var decoded = try loadEncryptedDocument()
        guard decoded.modelIdentifier == speakerIdentificationModelIdentifier,
              decoded.preprocessingVersion == speakerIdentificationPreprocessingVersion else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        guard ([speakerIdentificationDecisionVersion] + speakerPreviousDecisionVersions)
            .contains(decoded.decisionVersion) else {
            throw SpeakerIdentificationFailure.decisionVersionMismatch
        }
        // v6のID・固定anchor・別名・保留証拠を維持し、次の通常保存で移行する。
        decoded.decisionVersion = speakerIdentificationDecisionVersion
        document = decoded
        do {
            try rebuildAliasIndex(from: decoded)
        } catch let error as SpeakerIdentificationFailure {
            throw error
        } catch {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
    }

    private func loadEncryptedDocument() throws -> SpeakerRegistryDocument {
        try Self.loadEncryptedDocument(at: path, keyStore: keyStore)
    }

    private static func loadEncryptedDocument(
        at path: URL,
        keyStore: SpeakerKeyStore
    ) throws -> SpeakerRegistryDocument {
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
            return decoded
        } catch let error as SpeakerIdentificationFailure {
            throw error
        } catch {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
    }

    // 統合・取り消し・表示名の UI が提示する候補の読み取り専用ビュー。
    // 台帳の保存は atomic なため、稼働中の台帳 lock を待たずに読む。
    static func managementDirectory(
        path: String,
        keyStore: SpeakerKeyStore? = nil
    ) throws -> SpeakerManagementDirectory? {
        guard !path.isEmpty else { throw SpeakerIdentificationFailure.ledgerPathMissing }
        let ledgerURL = URL(fileURLWithPath: path)
        let store = keyStore ?? SpeakerKeychain(path: ledgerURL)
        guard Self.fileExists(at: ledgerURL) else {
            if try store.readExisting() != nil {
                throw SpeakerIdentificationFailure.ledgerMissing
            }
            return nil
        }
        let decoded = try loadEncryptedDocument(at: ledgerURL, keyStore: store)
        guard decoded.modelIdentifier == speakerIdentificationModelIdentifier,
              decoded.preprocessingVersion == speakerIdentificationPreprocessingVersion else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        guard ([speakerIdentificationDecisionVersion] + speakerPreviousDecisionVersions)
            .contains(decoded.decisionVersion) else {
            throw SpeakerIdentificationFailure.decisionVersionMismatch
        }
        return SpeakerManagementDirectory(
            registryID: decoded.registryID,
            speakers: decoded.profiles
                .filter { $0.state == "active" && decoded.aliases[$0.id] == nil }
                .map(\.id)
                .sorted(),
            aliases: decoded.aliases
        )
    }

    // 判定版の不一致で拒否された台帳を、明示的な全削除操作のために
    // 空の現行版へ置き換える。有効な暗号化台帳であることを検証し、
    // 台帳 ID は保持する。破損・鍵欠落・モデル不一致の台帳は
    // 復旧せず拒否する。
    static func managementDeleteAll(path: String, keyStore: SpeakerKeyStore? = nil) throws {
        do {
            let ledger = try SpeakerLedger(path: path, keyStore: keyStore)
            try ledger.deleteAll()
        } catch let error as SpeakerIdentificationFailure {
            guard case .decisionVersionMismatch = error else { throw error }
            let ledger = try SpeakerLedger(
                path: path,
                keyStore: keyStore,
                modelPackageDigest: nil,
                beforePersistence: {},
                afterPersistenceBegan: {},
                afterCommitReserved: {},
                shouldFailAliasWrite: { false },
                clock: { Date() },
                loadExistingDocument: false
            )
            try ledger.replaceDocumentAfterDecisionVersionMismatch()
            try ledger.deleteAll()
        }
    }

    private func replaceDocumentAfterDecisionVersionMismatch() throws {
        guard Self.fileExists(at: path) else {
            if try keyStore.readExisting() != nil {
                throw SpeakerIdentificationFailure.ledgerMissing
            }
            document = nil
            return
        }
        let decoded = try loadEncryptedDocument()
        guard decoded.modelIdentifier == speakerIdentificationModelIdentifier,
              decoded.preprocessingVersion == speakerIdentificationPreprocessingVersion else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        // 読み直しの間に現行版へ置き換わっていた場合は、そのまま削除へ進む
        guard decoded.decisionVersion != speakerIdentificationDecisionVersion else {
            document = decoded
            return
        }
        document = SpeakerRegistryDocument(
            schemaVersion: 2,
            registryID: decoded.registryID,
            modelIdentifier: decoded.modelIdentifier,
            modelPackageDigest: decoded.modelPackageDigest,
            preprocessingVersion: decoded.preprocessingVersion,
            decisionVersion: speakerIdentificationDecisionVersion,
            profiles: [],
            aliases: [:],
            pendingCandidates: [],
            backfillEvidence: [],
            pendingCorrections: [],
            revision: decoded.revision
        )
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

    private static func validate(_ document: SpeakerRegistryDocument) throws {
        var profileIDs = Set<String>()
        guard document.schemaVersion == 2,
              UUID(uuidString: document.registryID) != nil,
              validSHA256(document.modelPackageDigest),
              document.profiles.count <= speakerMaximumProfiles,
              document.pendingCandidates.count <= speakerMaximumPendingCandidates,
              document.backfillEvidence.count <= speakerMaximumBackfillEvidence,
              document.trustedSamples.count <= speakerMaximumProfiles * speakerMaximumTrustedSamplesPerID,
              document.pendingCorrections.count <= speakerMaximumPendingCorrections,
              document.profiles.allSatisfy({
                  validSpeakerID($0.id)
                      && profileIDs.insert($0.id).inserted
                      && ($0.state == "active" || $0.state == "retired")
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
        guard document.pendingCandidates.allSatisfy({ candidate in
            candidate.embedding.count == 256
                && candidate.embedding.allSatisfy { $0.isFinite }
                && candidate.voicedFrameCount > 0
                && candidate.voicedFrameCount <= speakerWindowFrameCount * 16
                && candidate.observedAt.timeIntervalSinceReferenceDate.isFinite
                && candidate.references.count <= speakerMaximumPendingReferences
                && (candidate.confirmedSpeakerID.map { validSpeakerID($0) && candidate.references.count == 1 } ?? true)
                && candidate.references.allSatisfy { reference in
                    UUID(uuidString: reference.segmentID) != nil
                        && reference.audioEndMilliseconds > reference.audioStartMilliseconds
                }
                && Set(candidate.references.map { reference in
                    "\(reference.segmentID):\(reference.audioStartMilliseconds):\(reference.audioEndMilliseconds)"
                }).count == candidate.references.count
        }) else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        guard document.backfillEvidence.allSatisfy({ evidence in
            evidence.registryID == document.registryID
                && evidence.modelPackageDigest == document.modelPackageDigest
                && validSHA256(evidence.modelPackageDigest)
                && UUID(uuidString: evidence.segmentID) != nil
                && evidence.audioEndMilliseconds > evidence.audioStartMilliseconds
                && evidence.embedding.count == 256
                && evidence.embedding.allSatisfy { $0.isFinite }
                && evidence.voicedFrameCount > 0
                && evidence.voicedFrameCount <= speakerWindowFrameCount * 16
                && evidence.evidenceWindowCount > 0
                && evidence.evidenceWindowCount <= 64
                && evidence.singleWindowSimilarityThreshold.isFinite
                && evidence.knownSimilarityThreshold.isFinite
                && evidence.marginThreshold.isFinite
                && evidence.marginThreshold >= 0
                && evidence.observedAt.timeIntervalSinceReferenceDate.isFinite
        }) else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        guard document.pendingCorrections.allSatisfy({ correction in
            UUID(uuidString: correction.segmentID) != nil
                && correction.audioEndMilliseconds > correction.audioStartMilliseconds
                && validSpeakerID(correction.speakerID)
                && UUID(uuidString: correction.registryID) != nil
                && correction.registryID == document.registryID
                && validSHA256(correction.modelPackageDigest)
                && correction.modelPackageDigest == document.modelPackageDigest
        }) else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        guard document.trustedSamples.allSatisfy({ sample in
            sample.registryID == document.registryID
                && sample.modelPackageDigest == document.modelPackageDigest
                && document.profiles.contains { $0.id == sample.anchorID && $0.state == "active" }
                && UUID(uuidString: sample.reference.segmentID) != nil
                && sample.reference.audioEndMilliseconds > sample.reference.audioStartMilliseconds
                && sample.embedding.count == 256 && sample.embedding.allSatisfy { $0.isFinite }
                && sample.anchorScore.isFinite && abs(sample.anchorScore) <= 1.001
                && sample.observedAt.timeIntervalSinceReferenceDate.isFinite
        }) else { throw SpeakerIdentificationFailure.ledgerCorrupt }
        let backfillKeys = document.backfillEvidence.map { evidence in
            "\(evidence.registryID):\(evidence.segmentID):\(evidence.audioStartMilliseconds):\(evidence.audioEndMilliseconds)"
        }
        guard Set(backfillKeys).count == backfillKeys.count else {
            throw SpeakerIdentificationFailure.ledgerCorrupt
        }
        let correctionKeys = document.pendingCorrections.map { correction in
            "\(correction.registryID):\(correction.segmentID):\(correction.audioStartMilliseconds):\(correction.audioEndMilliseconds):\(correction.speakerID)"
        }
        guard Set(correctionKeys).count == correctionKeys.count else {
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
        var samplesBySpeaker: [String: [TrustedSpeakerSample]] = [:]
        for sample in document.trustedSamples {
            var id = sample.anchorID
            while let next = document.aliases[id] { id = next }
            samplesBySpeaker[id, default: []].append(sample)
        }
        guard samplesBySpeaker.values.allSatisfy({ samples in
            samples.count <= speakerMaximumTrustedSamplesPerID
                && samples.enumerated().allSatisfy { index, sample in
                    !samples.prefix(index).contains { previous in
                        previous.reference.segmentID == sample.reference.segmentID
                    }
                }
        }) else { throw SpeakerIdentificationFailure.ledgerCorrupt }
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

    private func pendingEvidenceReference(
        segmentID: String?,
        segmentStartMilliseconds: UInt64?,
        startSample: Int,
        endSample: Int
    ) -> PendingSpeakerEvidenceReference? {
        guard let segmentID,
              UUID(uuidString: segmentID) != nil,
              let segmentStartMilliseconds,
              endSample > startSample else {
            return nil
        }
        let startMilliseconds = segmentStartMilliseconds
            + UInt64(startSample) * 1_000 / UInt64(speakerSampleRate)
        let endMilliseconds = max(
            segmentStartMilliseconds
                + UInt64(endSample) * 1_000 / UInt64(speakerSampleRate),
            startMilliseconds + 1
        )
        return PendingSpeakerEvidenceReference(
            segmentID: segmentID,
            audioStartMilliseconds: startMilliseconds,
            audioEndMilliseconds: endMilliseconds
        )
    }

    private func uniqueCorrections(
        _ corrections: [SpeakerIdentificationCorrection]
    ) -> [SpeakerIdentificationCorrection] {
        var seen = Set<String>()
        return corrections.filter { correction in
            let key = [
                correction.segmentID,
                String(correction.audioStartMilliseconds),
                String(correction.audioEndMilliseconds),
                correction.speakerID,
                correction.registryID,
            ].joined(separator: ":")
            return seen.insert(key).inserted
        }
    }

    private func pendingCandidateEmbedding(_ candidate: PendingSpeakerCandidate)
        -> SpeakerEmbeddingWindow {
        SpeakerEmbeddingWindow(
            embedding: candidate.embedding,
            startSample: 0,
            voicedFrames: Set(0..<max(candidate.voicedFrameCount, 1))
        )
    }

    private func backfillEvidenceKey(
        registryID: String,
        segmentID: String,
        audioStartMilliseconds: UInt64,
        audioEndMilliseconds: UInt64
    ) -> String {
        "\(registryID):\(segmentID):\(audioStartMilliseconds):\(audioEndMilliseconds)"
    }

    // 保存済みイベントの再送だけを行う。ここでは埋め込み比較や新しい訂正の
    // 判定をしないため、現行recentの根拠集合には戻らない。
    private func replayablePendingCorrections() -> [SpeakerIdentificationCorrection] {
        guard let currentDocument = document else { return [] }
        let activeIDs = Set(
            currentDocument.profiles
                .filter { $0.state == "active" }
                .map { canonicalID($0.id) }
        )
        return uniqueCorrections(currentDocument.pendingCorrections.compactMap { correction in
            guard correction.registryID == currentDocument.registryID else { return nil }
            let canonicalSpeakerID = canonicalID(correction.speakerID)
            guard activeIDs.contains(canonicalSpeakerID) else { return nil }
            return SpeakerIdentificationCorrection(
                segmentID: correction.segmentID,
                audioStartMilliseconds: correction.audioStartMilliseconds,
                audioEndMilliseconds: correction.audioEndMilliseconds,
                speakerID: canonicalSpeakerID,
                registryID: currentDocument.registryID,
                modelPackageDigest: currentDocument.modelPackageDigest,
                decisionDetails: correction.decisionDetails
            )
        })
    }

    private func appendPendingCorrections(
        _ corrections: [SpeakerIdentificationCorrection],
        to document: inout SpeakerRegistryDocument
    ) {
        let combined = uniqueCorrections(document.pendingCorrections + corrections)
        document.pendingCorrections = Array(
            combined.suffix(speakerMaximumPendingCorrections)
        )
    }

    private func correctedReferences(in document: SpeakerRegistryDocument) -> Set<PendingSpeakerEvidenceReference> {
        Set(document.pendingCorrections.map {
            PendingSpeakerEvidenceReference(
                segmentID: $0.segmentID, audioStartMilliseconds: $0.audioStartMilliseconds,
                audioEndMilliseconds: $0.audioEndMilliseconds
            )
        })
    }

    private func rememberBackfillEvidence(
        from analysis: SpeakerDecisionAnalysis,
        reference: PendingSpeakerEvidenceReference?,
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?
    ) throws {
        guard let reference,
              !analysis.mixed,
              !analysis.allEvidenceWindows.isEmpty,
              analysis.voicedFrames.count * speakerFrameShiftSampleCount
                  >= speakerMinimumSpeechSamples else {
            return
        }
        if let document, correctedReferences(in: document).contains(reference) { return }
        guard let modelPackageDigest = expectedModelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        let now = clock()
        var updated = document ?? SpeakerRegistryDocument(
            schemaVersion: 2,
            registryID: UUID().uuidString.lowercased(),
            modelIdentifier: speakerIdentificationModelIdentifier,
            modelPackageDigest: modelPackageDigest,
            preprocessingVersion: speakerIdentificationPreprocessingVersion,
            decisionVersion: speakerIdentificationDecisionVersion,
            profiles: [],
            aliases: [:],
            pendingCandidates: [],
            backfillEvidence: [],
            pendingCorrections: [],
            revision: 0
        )
        guard updated.modelPackageDigest == modelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        let retained = updated.backfillEvidence.filter { evidence in
            evidence.registryID == updated.registryID
                && evidence.modelPackageDigest == updated.modelPackageDigest
                && now.timeIntervalSince(evidence.observedAt)
                    <= speakerBackfillEvidenceLifetimeSeconds
        }
        let evidence = SpeakerBackfillEvidence(
            registryID: updated.registryID,
            modelPackageDigest: updated.modelPackageDigest,
            segmentID: reference.segmentID,
            audioStartMilliseconds: reference.audioStartMilliseconds,
            audioEndMilliseconds: reference.audioEndMilliseconds,
            embedding: analysis.segmentEmbedding,
            voicedFrameCount: analysis.voicedFrames.count,
            evidenceWindowCount: analysis.evidenceWindows.count,
            singleWindowSimilarityThreshold: analysis.singleWindowSimilarityThreshold,
            knownSimilarityThreshold: analysis.knownSimilarityThreshold,
            marginThreshold: analysis.marginThreshold,
            observedAt: now
        )
        var evidenceList = retained
        if !evidenceList.contains(where: { existing in
            backfillEvidenceKey(
                registryID: existing.registryID,
                segmentID: existing.segmentID,
                audioStartMilliseconds: existing.audioStartMilliseconds,
                audioEndMilliseconds: existing.audioEndMilliseconds
            ) == backfillEvidenceKey(
                registryID: evidence.registryID,
                segmentID: evidence.segmentID,
                audioStartMilliseconds: evidence.audioStartMilliseconds,
                audioEndMilliseconds: evidence.audioEndMilliseconds
            )
        }) {
            evidenceList.append(evidence)
        }
        evidenceList.sort { $0.observedAt < $1.observedAt }
        if evidenceList.count > speakerMaximumBackfillEvidence {
            evidenceList.removeFirst(evidenceList.count - speakerMaximumBackfillEvidence)
        }
        updated.backfillEvidence = evidenceList
        pruneBackfillEvidence(in: &updated, now: now)
        updated.revision &+= 1
        try save(
            updated,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        document = updated
    }

    private func pruneBackfillEvidence(in updated: inout SpeakerRegistryDocument, now: Date) {
        updated.backfillEvidence.removeAll {
            now.timeIntervalSince($0.observedAt) > speakerBackfillEvidenceLifetimeSeconds
        }
        updated.backfillEvidence.sort { $0.observedAt < $1.observedAt }
        if updated.backfillEvidence.count > speakerMaximumBackfillEvidence {
            updated.backfillEvidence.removeFirst(updated.backfillEvidence.count - speakerMaximumBackfillEvidence)
        }
    }

    private func supportingSamples(
        for evidence: SpeakerBackfillEvidence,
        samples: [TrustedSpeakerSample]
    ) -> [SpeakerSupportingSample] {
        let eligible = samples.filter {
            $0.registryID == evidence.registryID && $0.modelPackageDigest == evidence.modelPackageDigest
                && $0.reference.segmentID != evidence.segmentID
        }
        // 固定の採用条件を通った直近3件のみ。対象unknownへの類似度では選別しない。
        var best: [SpeakerSupportingSample] = []
        var bestScore: Float = -1
        for left in eligible.indices {
            for right in eligible.indices where right > left {
                let a = eligible[left], b = eligible[right]
                guard a.anchorID == b.anchorID,
                      a.reference.segmentID != b.reference.segmentID,
                      cosineSimilarity(a.embedding, b.embedding) >= speakerTrustedSampleSimilarity else { continue }
                let aScore = cosineSimilarity(evidence.embedding, a.embedding)
                let bScore = cosineSimilarity(evidence.embedding, b.embedding)
                let score = min(aScore, bScore)
                guard score > bestScore else { continue }
                bestScore = score
                best = [(a, aScore), (b, bScore)].map { sample, score in
                    SpeakerSupportingSample(
                        segmentId: sample.reference.segmentID,
                        startMs: sample.reference.audioStartMilliseconds, endMs: sample.reference.audioEndMilliseconds,
                        anchorId: sample.anchorID, anchorScore: sample.anchorScore, score: score
                    )
                }
            }
        }
        return best
    }

    // 旧 allPairwise 診断の互換経路。現行recentの新規判定・訂正からは呼ばない。
    private func backfillStoredEvidenceForLegacyDiagnosis(
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?
    ) throws -> [SpeakerIdentificationCorrection] {
        guard let originalDocument = document else { return [] }
        let now = clock()
        var currentDocument = originalDocument
        pruneBackfillEvidence(in: &currentDocument, now: now)
        var retainedEvidence = currentDocument.backfillEvidence
        var matchedKeys = Set<String>()
        var corrections: [SpeakerIdentificationCorrection] = []
        let speakerIDs = Set(currentDocument.profiles.filter { $0.state == "active" }.map { canonicalID($0.id) })
        for evidence in retainedEvidence {
            let ranked = speakerIDs.map { id -> (id: String, score: Float, support: [SpeakerSupportingSample]) in
                let support = supportingSamples(for: evidence, samples: currentDocument.trustedSamples.filter {
                    canonicalID($0.anchorID) == id
                })
                return (id, support.map(\.score).min() ?? -1, support)
            }.sorted { $0.score > $1.score }
            guard let first = ranked.first, first.support.count == 2,
                  first.score >= speakerTrustedSampleSimilarity,
                  ranked.count == 1 || first.score - ranked[1].score >= speakerTrustedSampleMargin else { continue }
            matchedKeys.insert(backfillEvidenceKey(
                registryID: evidence.registryID, segmentID: evidence.segmentID,
                audioStartMilliseconds: evidence.audioStartMilliseconds,
                audioEndMilliseconds: evidence.audioEndMilliseconds
            ))
            corrections.append(SpeakerIdentificationCorrection(
                segmentID: evidence.segmentID,
                audioStartMilliseconds: evidence.audioStartMilliseconds,
                audioEndMilliseconds: evidence.audioEndMilliseconds,
                speakerID: first.id, registryID: currentDocument.registryID,
                modelPackageDigest: currentDocument.modelPackageDigest,
                decisionDetails: SpeakerDecisionDetails(
                    startMs: evidence.audioStartMilliseconds, endMs: evidence.audioEndMilliseconds,
                    decisionVersion: speakerIdentificationDecisionVersion, registryId: currentDocument.registryID,
                    modelPackageDigest: currentDocument.modelPackageDigest, phase: "backfill-samples",
                    status: .identified, reason: "matched-samples",
                    candidates: ranked.prefix(3).map { SpeakerCandidateScore(speakerId: $0.id, score: $0.score) },
                    candidateCount: speakerIDs.count, knownThreshold: speakerTrustedSampleSimilarity,
                    marginThreshold: speakerTrustedSampleMargin, evidenceWindowCount: evidence.evidenceWindowCount,
                    voicedFrameCount: evidence.voicedFrameCount, supportingSamples: first.support,
                    supportThreshold: speakerTrustedSampleSimilarity
                )
            ))
        }
        let evidenceChanged = retainedEvidence != originalDocument.backfillEvidence
            || currentDocument.trustedSamples != originalDocument.trustedSamples
        guard evidenceChanged || !corrections.isEmpty else { return [] }
        retainedEvidence.removeAll { evidence in
            matchedKeys.contains(
                backfillEvidenceKey(
                    registryID: evidence.registryID,
                    segmentID: evidence.segmentID,
                    audioStartMilliseconds: evidence.audioStartMilliseconds,
                    audioEndMilliseconds: evidence.audioEndMilliseconds
                )
            )
        }
        var updated = currentDocument
        updated.backfillEvidence = retainedEvidence
        appendPendingCorrections(corrections, to: &updated)
        let corrected = correctedReferences(in: updated)
        updated.pendingCandidates.removeAll {
            $0.confirmedSpeakerID == nil && $0.references.contains(where: corrected.contains)
        }
        updated.revision &+= 1
        try save(
            updated,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        document = updated
        return uniqueCorrections(corrections)
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

    private func savePendingCandidates(
        _ candidates: [PendingSpeakerCandidate],
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?,
        maximumCount: Int = speakerMaximumPendingCandidates
    ) throws {
        guard let modelPackageDigest = expectedModelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        var updated = document ?? SpeakerRegistryDocument(
            schemaVersion: 2,
            registryID: UUID().uuidString.lowercased(),
            modelIdentifier: speakerIdentificationModelIdentifier,
            modelPackageDigest: modelPackageDigest,
            preprocessingVersion: speakerIdentificationPreprocessingVersion,
            decisionVersion: speakerIdentificationDecisionVersion,
            profiles: [],
            aliases: [:],
            pendingCandidates: [],
            backfillEvidence: [],
            pendingCorrections: [],
            revision: 0
        )
        guard updated.modelPackageDigest == modelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        updated.pendingCandidates = Array(candidates.prefix(maximumCount))
        updated.revision &+= 1
        try save(
            updated,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        document = updated
    }

    private func independentCandidates(_ left: PendingSpeakerCandidate, _ right: PendingSpeakerCandidate) -> Bool {
        if let a = left.references.first, let b = right.references.first {
            return a.segmentID != b.segmentID
        }
        return left.references.isEmpty && right.references.isEmpty && left.generation != right.generation
    }

    private func comparableRecentCandidates(_ left: PendingSpeakerCandidate, _ right: PendingSpeakerCandidate) -> Bool {
        if let a = left.references.first, let b = right.references.first {
            return !overlapsWithinSegment(a, b)
        }
        return left.references.isEmpty && right.references.isEmpty && left.generation != right.generation
    }

    private func overlapsWithinSegment(
        _ left: PendingSpeakerEvidenceReference,
        _ right: PendingSpeakerEvidenceReference
    ) -> Bool {
        left.segmentID == right.segmentID
            && left.audioStartMilliseconds < right.audioEndMilliseconds
            && right.audioStartMilliseconds < left.audioEndMilliseconds
    }

    private func identifyRecentCandidate(
        analysis: SpeakerDecisionAnalysis,
        generation: Int,
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?,
        evidenceReference: PendingSpeakerEvidenceReference?
    ) throws -> SpeakerIdentificationDecision {
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        let now = clock()
        let corrected = document.map { correctedReferences(in: $0) } ?? []
        let activeIDs = Set((document?.profiles ?? []).filter { $0.state == "active" }.map {
            canonicalID($0.id)
        })
        let recentPendingCandidates = (document?.pendingCandidates ?? []).filter {
            now.timeIntervalSince($0.observedAt) >= 0
                && now.timeIntervalSince($0.observedAt) <= speakerRecentCandidateLifetimeSeconds
                && $0.references.count <= 1
        }
        // 移動平均だった旧候補や、バックフィルだけで救済した発言は直接支持へ使わない。
        var pending = recentPendingCandidates.filter {
            ($0.confirmedSpeakerID.map { activeIDs.contains(canonicalID($0)) } ?? true)
                && ($0.confirmedSpeakerID != nil || !$0.references.contains(where: corrected.contains))
        }
        if let reference = evidenceReference, corrected.contains(reference) {
            try savePendingCandidates(pending, canCommit: canCommit, authorizeCommit: authorizeCommit)
            return SpeakerIdentificationDecision(
                speakerID: nil, registryID: nil, status: .unknown, reason: .pendingCandidate,
                corrections: [], recentCandidateCount: 0, recentMatchCount: 0
            )
        }
        let candidate = PendingSpeakerCandidate(
            generation: generation, embedding: analysis.segmentEmbedding,
            voicedFrameCount: analysis.voicedFrames.count, observedAt: now,
            references: evidenceReference.map { [$0] } ?? [], confirmedSpeakerID: nil
        )
        let comparable = pending.reversed().filter { comparableRecentCandidates($0, candidate) }
        let comparisons = comparable.map { prior in
            (candidate: prior, score: cosineSimilarity(prior.embedding, candidate.embedding))
        }
        let similar = comparisons.filter { $0.score >= speakerTrustedSampleSimilarity }.map { $0.candidate }
        let recentComparisons = comparisons.compactMap { comparison -> SpeakerRecentComparison? in
            guard let reference = comparison.candidate.references.first else { return nil }
            return SpeakerRecentComparison(
                segmentId: reference.segmentID,
                startMs: reference.audioStartMilliseconds,
                endMs: reference.audioEndMilliseconds,
                score: comparison.score
            )
        }
        let duplicate = recentPendingCandidates.contains {
            if let reference = evidenceReference {
                return $0.references.contains { overlapsWithinSegment($0, reference) }
            }
            return $0.references.isEmpty && $0.generation == generation
        }
        let recentBestScore = comparisons.map { $0.score }.max()
        let recentIDObservations = comparisons.compactMap {
            comparison -> (id: String, score: Float, candidate: PendingSpeakerCandidate)? in
            guard let confirmedSpeakerID = comparison.candidate.confirmedSpeakerID else { return nil }
            let id = canonicalID(confirmedSpeakerID)
            guard activeIDs.contains(id) else {
                return nil
            }
            return (id: id, score: comparison.score, candidate: comparison.candidate)
        }
        var bestRecentByID: [String: (score: Float, candidate: PendingSpeakerCandidate)] = [:]
        for observation in recentIDObservations {
            if bestRecentByID[observation.id].map({ $0.score >= observation.score }) ?? false {
                continue
            }
            bestRecentByID[observation.id] = (observation.score, observation.candidate)
        }
        let rankedRecentIDs = bestRecentByID.map {
            (id: $0.key, score: $0.value.score, candidate: $0.value.candidate)
        }.sorted {
            if $0.score != $1.score { return $0.score > $1.score }
            return $0.id < $1.id
        }
        let directIDs = Set(rankedRecentIDs.filter {
            $0.score >= speakerRecentDirectSimilarityThreshold
        }.map(\.id))
        let recentCandidateCount = comparable.count
        let recentMatchCount = comparisons.filter {
            $0.score >= speakerRecentDirectSimilarityThreshold
        }.count
        if directIDs.count > 1 {
            if !duplicate { pending.append(candidate) }
            if pending.count > speakerMaximumPendingCandidates {
                pending.removeFirst(pending.count - speakerMaximumPendingCandidates)
            }
            try savePendingCandidates(pending, canCommit: canCommit, authorizeCommit: authorizeCommit)
            return SpeakerIdentificationDecision(
                speakerID: nil, registryID: nil, status: .mixed, reason: .mixedClusters, corrections: [],
                candidateScores: [], recentCandidateCount: recentCandidateCount,
                recentMatchCount: recentMatchCount, recentBestScore: recentBestScore,
                recentComparisons: recentComparisons
            )
        }
        let directMatch: (id: String, score: Float, candidate: PendingSpeakerCandidate)?
        if directIDs.count == 1,
           let direct = rankedRecentIDs.first,
           direct.score >= speakerRecentDirectSimilarityThreshold,
           (rankedRecentIDs.count == 1
               || direct.score - rankedRecentIDs[1].score >= speakerRecentDirectMarginThreshold) {
            directMatch = direct
        } else {
            directMatch = nil
        }

        let isNovelComparedWithRecent = comparisons.isEmpty || comparisons.allSatisfy {
            $0.score <= analysis.newSpeakerSimilarityThreshold
        }
        if directMatch == nil && !duplicate && analysis.canEnroll && isNovelComparedWithRecent {
            let id = try registerLegacyProfile(
                embedding: analysis.segmentEmbedding,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit,
                pendingCandidates: pending,
                pendingCandidateLimit: speakerMaximumPendingCandidates,
                seedCandidate: candidate
            )
            return SpeakerIdentificationDecision(
                speakerID: id, registryID: document?.registryID, status: .identified,
                reason: .enrolledNew, corrections: [], candidateScores: [],
                recentCandidateCount: recentCandidateCount, recentMatchCount: recentMatchCount,
                recentBestScore: recentBestScore, recentComparisons: recentComparisons
            )
        }

        let decision = SpeakerIdentificationDecision(
            speakerID: nil, registryID: nil, status: .unknown, reason: .pendingCandidate, corrections: [],
            candidateScores: [], recentCandidateCount: recentCandidateCount,
            recentMatchCount: recentMatchCount, recentBestScore: recentBestScore,
            recentComparisons: recentComparisons
        )
        var cluster: [PendingSpeakerCandidate] = []
        if !duplicate {
            for (index, left) in similar.enumerated() {
                guard cluster.isEmpty else { break }
                for right in similar.dropFirst(index + 1) {
                    guard independentCandidates(left, right),
                          independentCandidates(left, candidate),
                          independentCandidates(right, candidate),
                          cosineSimilarity(left.embedding, right.embedding) >= speakerTrustedSampleSimilarity,
                          (left.voicedFrameCount + right.voicedFrameCount + candidate.voicedFrameCount)
                              * speakerFrameShiftSampleCount >= speakerEnrollmentMinimumSpeechSamples
                    else { continue }
                    if let directMatch {
                        // 既存IDへの通常一致がある場合、未確定候補だけの群で別IDを発行しない。
                        let hasDirectSupport = [left, right].contains { observation in
                            guard let confirmedSpeakerID = observation.confirmedSpeakerID else { return false }
                            return canonicalID(confirmedSpeakerID) == directMatch.id
                        }
                        guard hasDirectSupport else { continue }
                    }
                    cluster = [left, right, candidate]
                    break
                }
            }
        }
        if !duplicate { pending.append(candidate) }
        if pending.count > speakerMaximumPendingCandidates { pending.removeFirst(pending.count - speakerMaximumPendingCandidates) }
        guard !cluster.isEmpty else {
            try savePendingCandidates(pending, canCommit: canCommit, authorizeCommit: authorizeCommit)
            if let direct = directMatch {
                let supportingSamples = direct.candidate.references.first.map { reference in
                    [SpeakerSupportingSample(
                        segmentId: reference.segmentID,
                        startMs: reference.audioStartMilliseconds,
                        endMs: reference.audioEndMilliseconds,
                        anchorId: direct.id,
                        anchorScore: direct.score,
                        score: direct.score
                    )]
                } ?? []
                return SpeakerIdentificationDecision(
                    speakerID: direct.id, registryID: document?.registryID, status: .identified,
                    reason: .matchedSamples, corrections: [], candidateScores: [],
                    supportingSamples: supportingSamples, recentCandidateCount: recentCandidateCount,
                    recentMatchCount: recentMatchCount, recentBestScore: recentBestScore,
                    recentComparisons: recentComparisons
                )
            }
            return decision
        }
        var updated = try writableDocument()
        pruneBackfillEvidence(in: &updated, now: now)

        let selectedRecentObservations = cluster.dropLast()
        let recentSupportScores = selectedRecentObservations.reduce(into: [String: [Float]]()) { scores, observation in
            guard let confirmedSpeakerID = observation.confirmedSpeakerID else { return }
            scores[canonicalID(confirmedSpeakerID), default: []].append(
                cosineSimilarity(candidate.embedding, observation.embedding)
            )
        }
        var directlySupportedRecentIDs = Set(recentSupportScores.keys)
        // 新規群がまだIDを持たない場合は、選択群の外側にあるIDを混ぜない。
        // 既存ID群を直接支持する群を選んだ場合だけ、別のactive ID群にも
        // 独立した直接支持が2件以上ある競合をmixedとして扱う。
        if !directlySupportedRecentIDs.isEmpty {
            let selectedReferences = Set(selectedRecentObservations.flatMap(\.references))
            let outsideSupport = similar.reduce(into: [String: [PendingSpeakerCandidate]]()) { support, observation in
                guard let confirmedSpeakerID = observation.confirmedSpeakerID,
                      let reference = observation.references.first,
                      !selectedReferences.contains(reference) else { return }
                support[canonicalID(confirmedSpeakerID), default: []].append(observation)
            }
            directlySupportedRecentIDs.formUnion(
                outsideSupport.compactMap { id, observations in
                    let segmentIDs = Set(observations.compactMap { $0.references.first?.segmentID })
                    return segmentIDs.count >= 2 ? id : nil
                }
            )
        }
        // 再登録・削除済みのIDは管理上の履歴であり、現行群への直接支持として
        // 競合させない。activeな直近確定IDだけが再利用候補になる。
        let recentConfirmedIDs = Set(directlySupportedRecentIDs.filter { confirmedID in
            updated.profiles.contains {
                $0.state == "active" && canonicalID($0.id) == confirmedID
            }
        })
        if recentConfirmedIDs.count > 1 {
            try savePendingCandidates(pending, canCommit: canCommit, authorizeCommit: authorizeCommit)
            return SpeakerIdentificationDecision(
                speakerID: nil, registryID: nil, status: .mixed, reason: .mixedClusters, corrections: [],
                recentCandidateCount: recentCandidateCount, recentMatchCount: recentMatchCount,
                recentBestScore: comparisons.map { $0.score }.max(), recentComparisons: recentComparisons
            )
        }

        // IDの再利用は、直近比較で直接支持された確定群だけを根拠にする。
        // profileのanchorやtrustedSamplesは、現在の候補の裁定には使わない。
        let matchedID = recentConfirmedIDs.first.flatMap { confirmedID in
            updated.profiles.first {
                $0.state == "active" && canonicalID($0.id) == confirmedID
            }.map { canonicalID($0.id) }
        }
        let clusterEmbedding = try normalizedEmbedding(
            weightedAverage(cluster.map(pendingCandidateEmbedding))
        )
        let id: String
        if let matchedID {
            id = matchedID
        } else {
            guard updated.profiles.count < speakerMaximumProfiles else {
                throw SpeakerIdentificationFailure.ledgerWrite("話者台帳の上限に達しました")
            }
            id = UUID().uuidString.lowercased()
            updated.profiles.append(SpeakerProfile(
                id: id, anchor: clusterEmbedding, centroid: clusterEmbedding,
                updateCount: 0, state: "active"
            ))
        }
        if let index = updated.profiles.firstIndex(where: { $0.id == id }),
           cluster.allSatisfy({ $0.references.count == 1 }) {
            updated.trustedSamples.removeAll { canonicalID($0.anchorID) == id }
            updated.trustedSamples.append(contentsOf: cluster.map {
                TrustedSpeakerSample(
                    registryID: updated.registryID, modelPackageDigest: updated.modelPackageDigest,
                    anchorID: id, reference: $0.references[0], embedding: $0.embedding,
                    anchorScore: cosineSimilarity($0.embedding, clusterEmbedding), observedAt: $0.observedAt
                )
            })
            updated.profiles[index].updateCount &+= 1
        }
        // 相互一致とID裁定を通った群全体を、バックフィルだけの救済と区別する。
        let confirmedReferences = Set(cluster.flatMap(\.references))
        for index in pending.indices where pending[index].references.count == 1
            && confirmedReferences.contains(pending[index].references[0]) {
            pending[index].confirmedSpeakerID = id
        }
        updated.pendingCandidates = pending
        pruneBackfillEvidence(in: &updated, now: now)
        let corrections = recentPendingCorrections(
            from: cluster,
            candidates: pending,
            document: updated
        )
        let correctedKeys = Set(corrections.map {
            backfillEvidenceKey(
                registryID: updated.registryID,
                segmentID: $0.segmentID,
                audioStartMilliseconds: $0.audioStartMilliseconds,
                audioEndMilliseconds: $0.audioEndMilliseconds
            )
        })
        if !correctedKeys.isEmpty {
            updated.backfillEvidence.removeAll { evidence in
                correctedKeys.contains(
                    backfillEvidenceKey(
                        registryID: evidence.registryID,
                        segmentID: evidence.segmentID,
                        audioStartMilliseconds: evidence.audioStartMilliseconds,
                        audioEndMilliseconds: evidence.audioEndMilliseconds
                    )
                )
            }
            appendPendingCorrections(corrections, to: &updated)
        }
        updated.revision &+= 1
        try save(updated, canCommit: canCommit, authorizeCommit: authorizeCommit)
        document = updated
        // 訂正の根拠も、今回の直近窓内にある直接一致だけに限定する。
        // 保存済みbackfillEvidenceや過去trusted sampleは、現行経路で再照合しない。
        let support = cluster.dropLast().compactMap { observation -> SpeakerSupportingSample? in
            guard let reference = observation.references.first else { return nil }
            return SpeakerSupportingSample(
                segmentId: reference.segmentID, startMs: reference.audioStartMilliseconds, endMs: reference.audioEndMilliseconds,
                anchorId: id, anchorScore: cosineSimilarity(observation.embedding, clusterEmbedding),
                score: cosineSimilarity(candidate.embedding, observation.embedding)
            )
        }
        return SpeakerIdentificationDecision(
            speakerID: id, registryID: updated.registryID, status: .identified,
            reason: matchedID == nil ? .enrolledPending : .matchedSamples, corrections: corrections,
            candidateScores: [], supportingSamples: support,
            recentCandidateCount: recentCandidateCount, recentMatchCount: recentMatchCount,
            recentBestScore: comparisons.map { $0.score }.max(), recentComparisons: recentComparisons
        )
    }

    private func recentPendingCorrections(
        from cluster: [PendingSpeakerCandidate],
        candidates: [PendingSpeakerCandidate],
        document: SpeakerRegistryDocument
    ) -> [SpeakerIdentificationCorrection] {
        let alreadyCorrected = correctedReferences(in: document)
        let currentReferences = Set(candidates.flatMap(\.references))
        let clusterReferences = Set(cluster.compactMap { $0.references.first })
        var targets = cluster.dropLast().filter {
            guard let reference = $0.references.first else { return false }
            return currentReferences.contains(reference)
        }
        targets.append(contentsOf: candidates.filter {
            guard let reference = $0.references.first else { return false }
            return !clusterReferences.contains(reference)
        })

        let activeSpeakerIDs = Set(document.profiles.filter { $0.state == "active" }.map {
            canonicalID($0.id)
        })
        var supportGroups: [String: [PendingSpeakerCandidate]] = [:]
        for candidate in candidates {
            guard candidate.references.count == 1,
                  let confirmedSpeakerID = candidate.confirmedSpeakerID else {
                continue
            }
            let canonicalSpeakerID = canonicalID(confirmedSpeakerID)
            guard activeSpeakerIDs.contains(canonicalSpeakerID) else { continue }
            supportGroups[canonicalSpeakerID, default: []].append(candidate)
        }

        func directSupportPair(
            for target: PendingSpeakerCandidate,
            in supportCandidates: [PendingSpeakerCandidate]
        ) -> (PendingSpeakerCandidate, PendingSpeakerCandidate)? {
            guard let targetReference = target.references.first else { return nil }
            for leftIndex in supportCandidates.indices {
                guard let leftReference = supportCandidates[leftIndex].references.first,
                      leftReference.segmentID != targetReference.segmentID,
                      cosineSimilarity(
                          target.embedding,
                          supportCandidates[leftIndex].embedding
                      ) >= speakerTrustedSampleSimilarity else {
                    continue
                }
                for rightIndex in supportCandidates.indices where rightIndex > leftIndex {
                    guard let rightReference = supportCandidates[rightIndex].references.first,
                          rightReference.segmentID != targetReference.segmentID,
                          leftReference.segmentID != rightReference.segmentID,
                          cosineSimilarity(
                              target.embedding,
                              supportCandidates[rightIndex].embedding
                          ) >= speakerTrustedSampleSimilarity,
                          cosineSimilarity(
                              supportCandidates[leftIndex].embedding,
                              supportCandidates[rightIndex].embedding
                          ) >= speakerTrustedSampleSimilarity else {
                        continue
                    }
                    return (supportCandidates[leftIndex], supportCandidates[rightIndex])
                }
            }
            return nil
        }

        return targets.compactMap { candidate in
            guard candidate.confirmedSpeakerID == nil,
                  let reference = candidate.references.first,
                  !alreadyCorrected.contains(reference) else {
                return nil
            }
            let validSupportGroups = supportGroups.compactMap {
                speakerID, supportCandidates in
                directSupportPair(for: candidate, in: supportCandidates).map {
                    (speakerID: speakerID, pair: $0)
                }
            }
            guard validSupportGroups.count == 1,
                  let validSupportGroup = validSupportGroups.first else {
                return nil
            }
            let supports = [validSupportGroup.pair.0, validSupportGroup.pair.1]
            let supportScore = cosineSimilarity(
                supports[0].embedding,
                supports[1].embedding
            )
            let supportingSamples = supports.compactMap { support -> SpeakerSupportingSample? in
                guard let supportReference = support.references.first else { return nil }
                return SpeakerSupportingSample(
                    segmentId: supportReference.segmentID,
                    startMs: supportReference.audioStartMilliseconds,
                    endMs: supportReference.audioEndMilliseconds,
                    anchorId: validSupportGroup.speakerID,
                    anchorScore: supportScore,
                    score: cosineSimilarity(candidate.embedding, support.embedding)
                )
            }
            let comparisons = supports.compactMap { support -> SpeakerRecentComparison? in
                guard let supportReference = support.references.first else { return nil }
                return SpeakerRecentComparison(
                    segmentId: supportReference.segmentID,
                    startMs: supportReference.audioStartMilliseconds,
                    endMs: supportReference.audioEndMilliseconds,
                    score: cosineSimilarity(candidate.embedding, support.embedding)
                )
            }
            return SpeakerIdentificationCorrection(
                segmentID: reference.segmentID,
                audioStartMilliseconds: reference.audioStartMilliseconds,
                audioEndMilliseconds: reference.audioEndMilliseconds,
                speakerID: validSupportGroup.speakerID,
                registryID: document.registryID,
                modelPackageDigest: document.modelPackageDigest,
                decisionDetails: SpeakerDecisionDetails(
                    startMs: reference.audioStartMilliseconds,
                    endMs: reference.audioEndMilliseconds,
                    decisionVersion: speakerIdentificationDecisionVersion,
                    registryId: document.registryID,
                    modelPackageDigest: document.modelPackageDigest,
                    phase: "initial-recent",
                    status: .identified,
                    reason: "recent-consensus",
                    candidates: [],
                    candidateCount: 0,
                    knownThreshold: speakerRecentDirectSimilarityThreshold,
                    marginThreshold: 0,
                    evidenceWindowCount: 1,
                    voicedFrameCount: candidate.voicedFrameCount,
                    supportingSamples: supportingSamples,
                    supportThreshold: speakerTrustedSampleSimilarity,
                    recentCandidateCount: comparisons.count,
                    recentMatchCount: comparisons.count,
                    recentBestScore: comparisons.map(\.score).max(),
                    recentComparisons: comparisons
                )
            )
        }
    }

    private func enrollOrRememberLegacyCandidate(
        from windows: [SpeakerEmbeddingWindow],
        generation: Int,
        consistencyThreshold: Float,
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?,
        evidenceReference: PendingSpeakerEvidenceReference?
    ) throws -> (String?, String?, SpeakerIdentificationStatusValue, [SpeakerIdentificationCorrection]) {
        guard let candidate = windows.first else { return (nil, nil, .unknown, []) }
        let now = clock()
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        var pending = document?.pendingCandidates ?? []
        let retained = Array(pending.filter {
            now.timeIntervalSince($0.observedAt) <= 60.0
        }.suffix(speakerMaximumLegacyPendingCandidates))
        if retained.count != pending.count {
            try savePendingCandidates(
                retained,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit,
                maximumCount: speakerMaximumLegacyPendingCandidates
            )
            pending = retained
        }
        if let index = pending.indices.max(by: {
            cosineSimilarity(pending[$0].embedding, candidate.embedding)
                < cosineSimilarity(pending[$1].embedding, candidate.embedding)
        }), cosineSimilarity(pending[index].embedding, candidate.embedding)
            >= consistencyThreshold {
            let previous = pending[index]
            let duplicateReference = evidenceReference.map { reference in
                previous.references.contains(reference)
            } ?? false
            let additionalFrameCount = duplicateReference
                ? 0
                : candidate.voicedFrames.count
            // 保留からの登録にも同じ有声量の下限を適用する。観測ごとの和集合は
            // 別々の音声なので合計し、同じ観測内の重なりは窓の構築時の和集合で
            // 既に除かれている。
            let mergedSpeechSamples = (previous.voicedFrameCount + additionalFrameCount)
                * speakerFrameShiftSampleCount
            if mergedSpeechSamples >= speakerEnrollmentMinimumSpeechSamples {
                let embedding = try normalizedEmbedding(weightedAverage([
                    pendingCandidateEmbedding(previous),
                    candidate,
                ]))
                let remaining = pending.enumerated()
                    .filter { $0.offset != index }
                    .map(\.element)
                let id = try registerLegacyProfile(
                    embedding: embedding,
                    canCommit: canCommit,
                    authorizeCommit: authorizeCommit,
                    pendingCandidates: remaining
                )
                // register が成功した場合は commit 済みなので、ここで deadline を再確認して
                // unavailable に戻すと、永続化済みの ID と応答が不一致になる。
                let corrections = try backfillStoredEvidenceForLegacyDiagnosis(
                    canCommit: canCommit,
                    authorizeCommit: authorizeCommit
                )
                return (
                    id,
                    document?.registryID,
                    .identified,
                    corrections
                )
            }

            var mergedReferences = previous.references
            if let evidenceReference, !duplicateReference {
                mergedReferences.append(evidenceReference)
            }
            if mergedReferences.count > speakerMaximumPendingReferences {
                mergedReferences.removeFirst(mergedReferences.count - speakerMaximumPendingReferences)
            }
            let mergedEmbedding = try normalizedEmbedding(weightedAverage([
                pendingCandidateEmbedding(previous),
                candidate,
            ]))
            pending[index] = PendingSpeakerCandidate(
                generation: previous.generation,
                embedding: mergedEmbedding,
                voicedFrameCount: previous.voicedFrameCount + additionalFrameCount,
                observedAt: now,
                references: mergedReferences, confirmedSpeakerID: nil
            )
            try savePendingCandidates(
                pending,
                canCommit: canCommit,
                authorizeCommit: authorizeCommit,
                maximumCount: speakerMaximumLegacyPendingCandidates
            )
            return (nil, nil, .unknown, [])
        }
        var references: [PendingSpeakerEvidenceReference] = []
        if let evidenceReference {
            references = [evidenceReference]
        }
        pending.append(
            PendingSpeakerCandidate(
                generation: generation,
                embedding: candidate.embedding,
                voicedFrameCount: candidate.voicedFrames.count,
                observedAt: now,
                references: references, confirmedSpeakerID: nil
            )
        )
        if pending.count > speakerMaximumLegacyPendingCandidates {
            pending.removeFirst(pending.count - speakerMaximumLegacyPendingCandidates)
        }
        try savePendingCandidates(
            pending,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit,
            maximumCount: speakerMaximumLegacyPendingCandidates
        )
        return (nil, nil, .unknown, [])
    }

    private func registerLegacyProfile(
        embedding: [Float],
        canCommit: () -> Bool,
        authorizeCommit: SpeakerLedgerCommitAuthorizer?,
        pendingCandidates: [PendingSpeakerCandidate]? = nil,
        pendingCorrections: [SpeakerIdentificationCorrection] = [],
        pendingCandidateLimit: Int = speakerMaximumLegacyPendingCandidates,
        seedCandidate: PendingSpeakerCandidate? = nil
    ) throws -> String {
        guard canCommit() else { throw SpeakerIdentificationFailure.deadlineExceeded }
        guard let modelPackageDigest = expectedModelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        var document = document ?? SpeakerRegistryDocument(
            schemaVersion: 2,
            registryID: UUID().uuidString.lowercased(),
            modelIdentifier: speakerIdentificationModelIdentifier,
            modelPackageDigest: modelPackageDigest,
            preprocessingVersion: speakerIdentificationPreprocessingVersion,
            decisionVersion: speakerIdentificationDecisionVersion,
            profiles: [],
            aliases: [:],
            pendingCandidates: [],
            backfillEvidence: [],
            pendingCorrections: [],
            revision: 0
        )
        guard document.profiles.count < speakerMaximumProfiles else {
            throw SpeakerIdentificationFailure.ledgerWrite("話者台帳の上限に達しました")
        }
        guard document.modelPackageDigest == modelPackageDigest else {
            throw SpeakerIdentificationFailure.modelPackageMismatch
        }
        appendPendingCorrections(pendingCorrections, to: &document)
        var id = UUID().uuidString.lowercased()
        while document.profiles.contains(where: { $0.id == id }) {
            id = UUID().uuidString.lowercased()
        }
        document.profiles.append(
            SpeakerProfile(
                id: id,
                anchor: embedding,
                centroid: embedding,
                updateCount: 0,
                state: "active"
            )
        )
        if let seedCandidate {
            let confirmedSeed = PendingSpeakerCandidate(
                generation: seedCandidate.generation,
                embedding: seedCandidate.embedding,
                voicedFrameCount: seedCandidate.voicedFrameCount,
                observedAt: seedCandidate.observedAt,
                references: seedCandidate.references,
                confirmedSpeakerID: id
            )
            let retainedCandidates = pendingCandidates ?? document.pendingCandidates
            document.pendingCandidates = Array(
                (retainedCandidates + [confirmedSeed]).suffix(pendingCandidateLimit)
            )
        } else if let pendingCandidates {
            document.pendingCandidates = Array(
                pendingCandidates.prefix(pendingCandidateLimit)
            )
        }
        document.revision &+= 1
        try save(
            document,
            canCommit: canCommit,
            authorizeCommit: authorizeCommit
        )
        self.document = document
        return id
    }

    private func nonOverlappingWindows(
        _ windows: [SpeakerEmbeddingWindow],
        strideSamples: Int
    ) -> [SpeakerEmbeddingWindow] {
        var selected: [SpeakerEmbeddingWindow] = []
        for window in windows.sorted(by: { $0.startSample < $1.startSample }) {
            guard selected.last.map({
                window.startSample >= $0.startSample + strideSamples
            }) ?? true else {
                continue
            }
            selected.append(window)
        }
        return selected
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

struct SpeakerIdentifiedWindowPeriod {
    let startSample: Int
    let endSample: Int
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
    let segmentEmbedding: [Float]?
    let decisionReason: SpeakerIdentificationDecisionReason
    let windowCount: Int
    let evidenceWindowCount: Int
    let candidateScores: [(id: String, score: Float)]
    let canEnroll: Bool
    var decisionDetails: [SpeakerDecisionDetails] = []
}

struct SpeakerIdentificationDecision {
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
    let reason: SpeakerIdentificationDecisionReason
    let corrections: [SpeakerIdentificationCorrection]
    var candidateScores: [SpeakerCandidateScore] = []
    var supportingSamples: [SpeakerSupportingSample] = []
    var recentCandidateCount: Int? = nil
    var recentMatchCount: Int? = nil
    var recentBestScore: Float? = nil
    var recentComparisons: [SpeakerRecentComparison]? = nil
}

struct SpeakerPeriodIdentification {
    let periods: [SpeakerIdentifiedWindowPeriod]
    let speakerID: String?
    let registryID: String?
    let status: SpeakerIdentificationStatusValue
    let corrections: [SpeakerIdentificationCorrection]
}

// 期間の境目は、直前の非重複窓（1 窓分以上前に開始した最も近い窓）との cosine が
// 境界しきい値を深く下回る位置に置く。候補は一つの交代の遷移帯にまとまって現れるため、
// cosine が最も低い窓から順に境目を確定し、確定した境目の 1 窓 + 1 ずらし幅以内の
// 残りの候補を捨てる。深い落ち込みのない部分（なだらかなドリフト）には境目を置かない。
// 境目は確定した窓とその前の窓の開始位置の中間に置く。
func speakerWindowPeriodRanges(
    _ windows: [SpeakerEmbeddingWindow],
    boundaryThreshold: Float = speakerPeriodBoundaryThreshold
) -> [(startSample: Int, endSample: Int, windows: [SpeakerEmbeddingWindow])] {
    let sorted = windows.sorted { $0.startSample < $1.startSample }
    guard !sorted.isEmpty else { return [] }
    guard sorted.count > 1 else {
        return [(
            startSample: sorted[0].startSample,
            endSample: sorted[0].startSample + speakerWindowSampleCount,
            windows: sorted
        )]
    }
    var candidates: [(index: Int, similarity: Float)] = []
    for index in 1..<sorted.count {
        guard let previousIndex = sorted.indices[..<index].last(where: {
            sorted[$0].startSample
                <= sorted[index].startSample - speakerWindowSampleCount
        }) else { continue }
        let similarity = cosineSimilarity(
            sorted[previousIndex].embedding,
            sorted[index].embedding
        )
        if similarity < boundaryThreshold {
            candidates.append((index: index, similarity: similarity))
        }
    }
    var boundaries: [Int] = []
    var remaining = candidates
    while let strongest = remaining.min(by: { $0.similarity < $1.similarity }) {
        boundaries.append(strongest.index)
        let strongestStart = sorted[strongest.index].startSample
        remaining.removeAll {
            abs(sorted[$0.index].startSample - strongestStart)
                < speakerPeriodBoundarySuppressionSampleCount
        }
    }
    boundaries.sort()
    var ranges: [(startSample: Int, endSample: Int, windows: [SpeakerEmbeddingWindow])] = []
    var currentStart = sorted[0].startSample
    var currentWindows: [SpeakerEmbeddingWindow] = []
    var boundaryIndex = 0
    for (index, window) in sorted.enumerated() {
        if boundaryIndex < boundaries.count, index == boundaries[boundaryIndex] {
            let boundary = (sorted[index - 1].startSample + window.startSample) / 2
            ranges.append((
                startSample: currentStart,
                endSample: boundary,
                windows: currentWindows
            ))
            currentStart = boundary
            currentWindows = []
            boundaryIndex += 1
        }
        currentWindows.append(window)
    }
    ranges.append((
        startSample: currentStart,
        endSample: (sorted.last?.startSample ?? 0) + speakerWindowSampleCount,
        windows: currentWindows
    ))
    return ranges
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
    let interval = Int((sampleRate * speakerWindowShiftSeconds).rounded())
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
    let voicedFrames = speakerSpeechFrames(window)
    let peak = window.map { abs($0) }.max() ?? 0
    guard voicedFrames.count * speakerFrameShiftSampleCount >= speakerMinimumSpeechSamples,
          peak < 1.0 else {
        return nil
    }
    let features = try WeSpeakerFbank.features(for: window)
    let embedding = try predictor.predict(features: features)
    return SpeakerEmbeddingWindow(
        embedding: embedding,
        startSample: Int(
            (Double(sourceStart) * speakerSampleRate / sampleRate).rounded()
        ),
        voicedFrames: voicedFrames
    )
}

func speakerEmbeddingWindows(
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

func speakerSpeechFrames(_ samples: [Float]) -> Set<Int> {
    let frameCount = samples.count / speakerFrameShiftSampleCount
    var voicedFrames = Set<Int>()
    for frame in 0..<frameCount {
        let start = frame * speakerFrameShiftSampleCount
        let rms = sqrt(
            samples[start..<(start + speakerFrameShiftSampleCount)]
                .reduce(Float.zero) { $0 + $1 * $1 }
                / Float(speakerFrameShiftSampleCount)
        )
        if rms >= 0.005 { voicedFrames.insert(frame) }
    }
    return voicedFrames
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

    func finishSegment(
        generation: Int,
        audioEndNanoseconds: UInt64,
        featureSpeechDurationNanoseconds: UInt64? = nil
    ) -> SpeakerIdentificationResult {
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
                var segment = segment
                if let featureSpeechDurationNanoseconds {
                    let samplesBeforeTrim = segment.samples.count
                    let speechSampleLimit = min(
                        samplesBeforeTrim,
                        Int(
                            min(
                                Double(Int.max),
                                Double(featureSpeechDurationNanoseconds)
                                    * segment.sampleRate / 1_000_000_000
                            ).rounded(.down)
                        )
                    )
                    if speechSampleLimit < samplesBeforeTrim {
                        segment.samples.removeLast(samplesBeforeTrim - speechSampleLimit)
                        let modelSpeechSampleLimit = Int(
                            min(
                                Double(Int.max),
                                Double(speechSampleLimit)
                                    * speakerSampleRate / segment.sampleRate
                            ).rounded(.down)
                        )
                        segment.windows.removeAll {
                            $0.startSample + speakerWindowSampleCount > modelSpeechSampleLimit
                        }
                        self.log(
                            "speaker-identification feature-trim generation=\(generation) "
                                + "samples-before=\(samplesBeforeTrim) "
                                + "samples-after=\(speechSampleLimit) "
                                + "windows=\(segment.windows.count)"
                        )
                    }
                }
                let decision = try self.ledger.identifyPeriods(
                    windows: segment.windows,
                    generation: generation,
                    canCommit: {
                        self.canCommit(generation: generation, before: deadline)
                    },
                    authorizeCommit: {
                        self.authorizeCommit(generation: generation, before: deadline)
                    },
                    segmentID: segment.segmentID,
                    segmentStartMilliseconds: start / 1_000_000
                )
                let segmentStartMilliseconds = start / 1_000_000
                result = SpeakerIdentificationResult(
                    segmentID: segment.segmentID,
                    audioStartMilliseconds: segmentStartMilliseconds,
                    audioEndMilliseconds: max(end / 1_000_000, start / 1_000_000 + 1),
                    speakerID: decision.speakerID,
                    registryID: decision.registryID,
                    status: decision.status,
                    periods: decision.periods.isEmpty ? [try self.ledger.noEvidencePeriod(
                        start: segmentStartMilliseconds, end: max(end / 1_000_000, segmentStartMilliseconds + 1)
                    )] : decision.periods.map { period in
                        let periodStart = segmentStartMilliseconds
                            + UInt64(period.startSample) * 1_000 / 16_000
                        let periodEnd = segmentStartMilliseconds
                            + UInt64(period.endSample) * 1_000 / 16_000
                        return SpeakerIdentificationPeriod(
                            audioStartMilliseconds: periodStart,
                            audioEndMilliseconds: max(periodEnd, periodStart + 1),
                            speakerID: period.speakerID,
                            registryID: period.registryID,
                            status: period.status,
                            decisionDetails: period.decisionDetails
                        )
                    },
                    corrections: decision.corrections
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
            status: .unavailable,
            periods: [],
            corrections: []
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

func speakerManagementDirectoryPayload(_ directory: SpeakerManagementDirectory?) -> [String: Any] {
    [
        "event": "speaker-directory",
        "registryId": directory.map { $0.registryID as Any } ?? NSNull(),
        "speakers": directory?.speakers ?? [],
        "aliases": directory?.aliases ?? [:],
    ]
}

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
                    "message": "統合元は --speaker-from <speaker UUID> で指定してください",
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
                    "message": "統合先は --speaker-to <speaker UUID> で指定してください",
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
                    "message": "話者 ID は --speaker-id <speaker UUID> で指定してください",
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
    if operation == "list" {
        guard sourceID == nil, targetID == nil, speakerID == nil else {
            emitSpeakerManagement([
                "event": "error",
                "kind": "arguments",
                "message": "一覧の取得には話者 ID を指定できません",
            ])
            exit(2)
        }
        do {
            let directory = try SpeakerLedger.managementDirectory(path: ledgerPath)
            emitSpeakerManagement(speakerManagementDirectoryPayload(directory))
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
    do {
        if operation == "delete-all" {
            guard sourceID == nil, targetID == nil, speakerID == nil else {
                throw SpeakerIdentificationFailure.ledgerWrite(
                    "全削除には話者 ID を指定できません"
                )
            }
            try SpeakerLedger.managementDeleteAll(path: ledgerPath)
        } else {
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
            default:
                emitSpeakerManagement([
                    "event": "error",
                    "kind": "arguments",
                    "message": "未対応の話者管理操作です: \(operation)",
                ])
                exit(2)
            }
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
    var periodBoundaryThreshold: Float?
    var enrollWindowCount: Int?
    var marginThreshold: Float?
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
        case "--speaker-period-threshold":
            guard periodBoundaryThreshold == nil,
                  index + 1 < arguments.count,
                  let value = Float(arguments[index + 1]),
                  value >= -1, value <= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-period-threshold は -1 以上 1 以下の数値を一度だけ指定してください"
                ))
            }
            periodBoundaryThreshold = value
            index += 2
        case "--speaker-enroll-windows":
            guard enrollWindowCount == nil,
                  index + 1 < arguments.count,
                  let value = Int(arguments[index + 1]),
                  value >= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-enroll-windows は 1 以上の整数を一度だけ指定してください"
                ))
            }
            enrollWindowCount = value
            index += 2
        case "--speaker-margin-threshold":
            guard marginThreshold == nil,
                  index + 1 < arguments.count,
                  let value = Float(arguments[index + 1]),
                  value >= 0, value <= 1 else {
                return .failure(SpeakerDiagnosisThresholdParseError(
                    message: "--speaker-margin-threshold は 0 以上 1 以下の数値を一度だけ指定してください"
                ))
            }
            marginThreshold = value
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
            windowConsistencyThreshold: windowConsistencyThreshold,
            periodBoundaryThreshold: periodBoundaryThreshold,
            enrollWindowCount: enrollWindowCount,
            marginThreshold: marginThreshold
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

    guard thresholdOverride.knownSimilarityThreshold == nil,
          thresholdOverride.newSpeakerSimilarityThreshold == nil,
          thresholdOverride.enrollWindowCount == nil,
          thresholdOverride.marginThreshold == nil else {
        emitSpeakerDiagnostic(
            "speaker-diagnose error=unsupported-v8-threshold-override "
                + "v8では旧anchor用のknown/new/enroll-windows/marginの上書きは使用できません。"
                + "品質・境界の調査には--speaker-window-thresholdと--speaker-period-thresholdを使用してください。"
        )
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
        let periodsLedger = try SpeakerLedger(
            forDiagnosisAt: diagnosisLedgerDirectory
                .appendingPathComponent("periods/registry.enc").path,
            modelPackageDigest: predictor.modelPackageDigest
        )
        let periodBoundaryThreshold = thresholdOverride.periodBoundaryThreshold
            ?? speakerPeriodBoundaryThreshold
        var legacySummary = SpeakerDiagnosticPolicySummary()
        var currentSummary = SpeakerDiagnosticPolicySummary()
        var allPairwiseSimilarities: [Float] = []
        allPairwiseSimilarities.reserveCapacity(files.count * 10)
        var segmentDurations: [Float] = []
        segmentDurations.reserveCapacity(files.count)
        var periodStatusCounts: [SpeakerIdentificationStatusValue: Int] = [:]
        var periodLevelStatusCounts: [SpeakerIdentificationStatusValue: Int] = [:]
        var periodReasonCounts: [SpeakerIdentificationDecisionReason: Int] = [:]
        var periodDurations: [Float] = []
        var periodIdentifiedEmbeddings: [(id: String, embedding: [Float])] = []
        var windowInferenceMilliseconds: [Float] = []
        windowInferenceMilliseconds.reserveCapacity(files.count)
        var diagnosisAudioMilliseconds: UInt64 = 0
        for (index, file) in files.enumerated() {
            let audio = try readSpeakerDiagnosticAudio(at: file)
            let diagnosisSegmentStart = diagnosisAudioMilliseconds
            diagnosisAudioMilliseconds += UInt64(ceil(Double(audio.samples.count) / audio.sampleRate * 1_000)) + 1
            segmentDurations.append(Float(Double(audio.samples.count) / audio.sampleRate))
            let inferenceStartedAt = DispatchTime.now().uptimeNanoseconds
            let windows = try speakerEmbeddingWindows(
                from: audio.samples,
                sampleRate: audio.sampleRate,
                predictor: predictor
            )
            let inferenceElapsedMilliseconds = Float(
                (DispatchTime.now().uptimeNanoseconds - inferenceStartedAt) / 1_000_000
            )
            if !windows.isEmpty {
                windowInferenceMilliseconds.append(
                    inferenceElapsedMilliseconds / Float(windows.count)
                )
            }
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
            let periods = try periodsLedger.identifyPeriods(
                windows: windows,
                boundaryThreshold: periodBoundaryThreshold,
                rule: currentRule,
                generation: index + 1,
                segmentID: UUID().uuidString.lowercased(),
                segmentStartMilliseconds: diagnosisSegmentStart
            )
            periodStatusCounts[periods.status, default: 0] += 1
            for period in periods.periods {
                periodLevelStatusCounts[period.status, default: 0] += 1
                periodDurations.append(
                    Float(period.endSample - period.startSample) / Float(speakerSampleRate)
                )
                if let id = period.speakerID, let embedding = period.segmentEmbedding {
                    periodIdentifiedEmbeddings.append((id: id, embedding: embedding))
                }
            }
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
                    + "window-inference-ms=\(speakerDiagnosticNumber(inferenceElapsedMilliseconds)) "
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
            let periodDetails = periods.periods.map { period in
                let start = Float(period.startSample) / Float(speakerSampleRate)
                let end = Float(period.endSample) / Float(speakerSampleRate)
                return "\(speakerDiagnosticNumber(start))-\(speakerDiagnosticNumber(end))"
                    + ":\(period.status.rawValue)"
                    + ":\(period.speakerID ?? "none")"
            }.joined(separator: ",")
            emitSpeakerDiagnostic(
                "speaker-diagnose-periods file=\(displayPath) "
                    + "count=\(periods.periods.count) "
                    + "overall=\(periods.status.rawValue) "
                    + "overall-speaker=\(periods.speakerID ?? "none") "
                    + "corrections=\(periods.corrections.count) "
                    + "periods=\(periodDetails.isEmpty ? "none" : periodDetails)"
            )
            for (periodIndex, period) in periods.periods.enumerated() {
                periodReasonCounts[period.decisionReason, default: 0] += 1
                let best = period.candidateScores.first
                let second = period.candidateScores.dropFirst().first
                let margin = best.flatMap { first in
                    second.map { first.score - $0.score }
                }
                emitSpeakerDiagnostic(
                    "speaker-diagnose-period file=\(displayPath) "
                        + "index=\(periodIndex) "
                        + "start=\(speakerDiagnosticNumber(Float(period.startSample) / Float(speakerSampleRate))) "
                        + "end=\(speakerDiagnosticNumber(Float(period.endSample) / Float(speakerSampleRate))) "
                        + "windows=\(period.windowCount) "
                        + "evidence-windows=\(period.evidenceWindowCount) "
                        + "status=\(period.status.rawValue) "
                        + "speaker=\(period.speakerID ?? "none") "
                        + "best=\(best.map { "\($0.id):\(speakerDiagnosticNumber($0.score))" } ?? "none") "
                        + "second=\(second.map { "\($0.id):\(speakerDiagnosticNumber($0.score))" } ?? "none") "
                        + "margin=\(speakerDiagnosticValue(margin)) "
                        + "can-enroll=\(period.canEnroll ? "yes" : "no") "
                        + "reason=\(period.decisionReason.rawValue) "
                        + "recent-candidates=\(period.decisionDetails.first?.recentCandidateCount ?? 0) "
                        + "recent-matches=\(period.decisionDetails.first?.recentMatchCount ?? 0) "
                        + "recent-best=\(speakerDiagnosticValue(period.decisionDetails.first?.recentBestScore))"
                )
            }
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
                + "override=\(thresholdOverride.hasOverride ? "yes" : "no") "
                + "centroid-update-threshold=\(speakerDiagnosticNumber(speakerCentroidUpdateSimilarityThreshold)) "
                + "centroid-update-margin=\(speakerDiagnosticNumber(speakerCentroidUpdateMarginThreshold)) "
                + "centroid-update-min-windows=\(speakerCentroidUpdateMinimumWindowCount) "
                + "centroid-auto-update=\(centroidAutoUpdateStatus) "
                + "window-shift-seconds=\(speakerWindowShiftSeconds) "
                + "evidence-stride-seconds=\(speakerDiagnosticNumber(Float(speakerEvidenceWindowSampleCount) / Float(speakerSampleRate))) "
                + "period-evidence-stride-seconds=\(speakerDiagnosticNumber(Float(speakerPeriodEvidenceWindowSampleCount) / Float(speakerSampleRate))) "
                + "period-boundary-threshold=\(speakerDiagnosticNumber(periodBoundaryThreshold)) "
                + "recent-window-seconds=\(speakerDiagnosticNumber(Float(speakerRecentCandidateLifetimeSeconds))) "
                + "recent-capacity=\(speakerMaximumPendingCandidates) "
                + "recent-threshold=\(speakerDiagnosticNumber(speakerTrustedSampleSimilarity)) "
                + "recent-margin=unused "
                + "stored-backfill-window-seconds=\(speakerDiagnosticNumber(Float(speakerBackfillEvidenceLifetimeSeconds))) "
                + "stored-backfill-capacity=\(speakerMaximumBackfillEvidence) "
                + "backfill-diagnostic-margin=\(speakerDiagnosticNumber(speakerTrustedSampleMargin)) "
                + "backfill=legacy-all-pairwise-only "
                + "matching=recent-independent-consensus"
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
        let periodDurationDistribution = speakerDiagnosticDistribution(periodDurations)
        let periodReasonSummary = SpeakerIdentificationDecisionReason.allCases.map {
            "reason-\($0.rawValue)=\(periodReasonCounts[$0, default: 0])"
        }.joined(separator: " ")
        emitSpeakerDiagnostic(
            "speaker-diagnose-periods-summary "
                + "identified=\(periodStatusCounts[.identified, default: 0]) "
                + "unknown=\(periodStatusCounts[.unknown, default: 0]) "
                + "mixed=\(periodStatusCounts[.mixed, default: 0]) "
                + "unavailable=\(periodStatusCounts[.unavailable, default: 0]) "
                + "period-identified=\(periodLevelStatusCounts[.identified, default: 0]) "
                + "period-unknown=\(periodLevelStatusCounts[.unknown, default: 0]) "
                + "period-mixed=\(periodLevelStatusCounts[.mixed, default: 0]) "
                + "registered-speakers=\(periodsLedger.activeProfileCount) "
                + periodReasonSummary
        )
        emitSpeakerDiagnostic(
            "speaker-diagnose-periods-duration-seconds "
                + "count=\(periodDurationDistribution.count) "
                + "short-under-1.5s=\(periodDurations.filter { $0 < 1.5 }.count) "
                + "min=\(speakerDiagnosticValue(periodDurationDistribution.minimum)) "
                + "p25=\(speakerDiagnosticValue(periodDurationDistribution.p25)) "
                + "median=\(speakerDiagnosticValue(periodDurationDistribution.median)) "
                + "p75=\(speakerDiagnosticValue(periodDurationDistribution.p75)) "
                + "max=\(speakerDiagnosticValue(periodDurationDistribution.maximum))"
        )
        let periodSameID = speakerDiagnosticDistribution(
            speakerDiagnosticSegmentSimilarities(periodIdentifiedEmbeddings, sameID: true)
        )
        let periodDifferentID = speakerDiagnosticDistribution(
            speakerDiagnosticSegmentSimilarities(periodIdentifiedEmbeddings, sameID: false)
        )
        emitSpeakerDiagnostic(
            "speaker-diagnose-periods-same-id "
                + "count=\(periodSameID.count) "
                + "min=\(speakerDiagnosticValue(periodSameID.minimum)) "
                + "p25=\(speakerDiagnosticValue(periodSameID.p25)) "
                + "median=\(speakerDiagnosticValue(periodSameID.median)) "
                + "p75=\(speakerDiagnosticValue(periodSameID.p75)) "
                + "max=\(speakerDiagnosticValue(periodSameID.maximum))"
        )
        emitSpeakerDiagnostic(
            "speaker-diagnose-periods-different-id "
                + "count=\(periodDifferentID.count) "
                + "min=\(speakerDiagnosticValue(periodDifferentID.minimum)) "
                + "p25=\(speakerDiagnosticValue(periodDifferentID.p25)) "
                + "median=\(speakerDiagnosticValue(periodDifferentID.median)) "
                + "p75=\(speakerDiagnosticValue(periodDifferentID.p75)) "
                + "max=\(speakerDiagnosticValue(periodDifferentID.maximum))"
        )
        let inferenceDistribution = speakerDiagnosticDistribution(windowInferenceMilliseconds)
        emitSpeakerDiagnostic(
            "speaker-diagnose-window-inference-milliseconds "
                + "count=\(inferenceDistribution.count) "
                + "min=\(speakerDiagnosticValue(inferenceDistribution.minimum)) "
                + "p25=\(speakerDiagnosticValue(inferenceDistribution.p25)) "
                + "median=\(speakerDiagnosticValue(inferenceDistribution.median)) "
                + "p75=\(speakerDiagnosticValue(inferenceDistribution.p75)) "
                + "max=\(speakerDiagnosticValue(inferenceDistribution.maximum))"
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
