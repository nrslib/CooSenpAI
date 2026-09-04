import Foundation

enum AudioSource: String, CaseIterable {
    case microphone
    case speaker
}

struct AudioVolumeMeasurement {
    let peak: Double
    let rms: Double
    let sampleCount: UInt64
}

struct AudioInputFailureTracker {
    // 連続した不正バッファを検出したら、該当 source だけを停止する。
    static let maximumConsecutiveFailures = 32

    private(set) var consecutiveFailures = 0

    mutating func recordFailure() -> Bool {
        if consecutiveFailures < Self.maximumConsecutiveFailures {
            consecutiveFailures += 1
        }
        return consecutiveFailures >= Self.maximumConsecutiveFailures
    }

    mutating func recordSuccessfulBuffer() {
        consecutiveFailures = 0
    }
}

private func formattedLevel(_ value: Double) -> String {
    String(format: "%.4f", value)
}

struct AudioStats {
    private(set) var buffers: UInt64 = 0
    private(set) var frames: UInt64 = 0
    private(set) var appends: UInt64 = 0
    private(set) var noSpeechRestarts: UInt64 = 0
    private(set) var noiseFloorRms: Double = 0
    private(set) var startRmsThreshold: Double = 0.002
    private(set) var sustainRmsThreshold: Double = 0.0008
    private var volumePeak: Double = 0
    private var volumeSquareSum: Double = 0
    private var volumeSampleCount: UInt64 = 0

    var peak: Double { volumePeak }

    var rms: Double {
        guard volumeSampleCount > 0 else { return 0 }
        return (volumeSquareSum / Double(volumeSampleCount)).squareRoot()
    }

    mutating func recordBuffer(frameCount: UInt64) {
        buffers &+= 1
        frames &+= frameCount
    }

    mutating func recordAppend() {
        appends &+= 1
    }

    mutating func recordNoSpeechRestart() {
        noSpeechRestarts &+= 1
    }

    mutating func recordVoiceActivity(
        noiseFloorRms: Double,
        startRmsThreshold: Double,
        sustainRmsThreshold: Double
    ) {
        self.noiseFloorRms = noiseFloorRms
        self.startRmsThreshold = startRmsThreshold
        self.sustainRmsThreshold = sustainRmsThreshold
    }

    mutating func recordVolume(_ measurement: AudioVolumeMeasurement) {
        guard measurement.sampleCount > 0 else { return }
        volumePeak = max(volumePeak, measurement.peak)
        volumeSquareSum += measurement.rms * measurement.rms * Double(measurement.sampleCount)
        volumeSampleCount &+= measurement.sampleCount
    }

    mutating func resetVolumeWindow() {
        volumePeak = 0
        volumeSquareSum = 0
        volumeSampleCount = 0
    }
}

struct AudioStatsSnapshot {
    let microphone: AudioStats
    let speaker: AudioStats
}

func audioStatsValue(
    for source: AudioSource,
    in snapshot: AudioStatsSnapshot
) -> AudioStats {
    switch source {
    case .microphone: return snapshot.microphone
    case .speaker: return snapshot.speaker
    }
}

func audioStatsLine(
    sources: Set<AudioSource>,
    snapshot: AudioStatsSnapshot
) -> String {
    AudioSource.allCases
        .filter { sources.contains($0) }
        .map { source in
            let stats = audioStatsValue(for: source, in: snapshot)
            return "\(source.rawValue) buffers=\(stats.buffers) frames=\(stats.frames) appends=\(stats.appends) noSpeechRestarts=\(stats.noSpeechRestarts) peak=\(formattedLevel(stats.peak)) rms=\(formattedLevel(stats.rms)) floor=\(formattedLevel(stats.noiseFloorRms)) start=\(formattedLevel(stats.startRmsThreshold)) sustain=\(formattedLevel(stats.sustainRmsThreshold))"
        }
        .joined(separator: " ")
}

func missingAudioSources(
    sources: Set<AudioSource>,
    snapshot: AudioStatsSnapshot
) -> [AudioSource] {
    AudioSource.allCases
        .filter { sources.contains($0) }
        .filter { audioStatsValue(for: $0, in: snapshot).buffers == 0 }
}
