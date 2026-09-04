import AVFoundation
import Foundation

func testAppendedAudioDump() throws {
    let directoryURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("coosenpai-appended-audio-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: directoryURL) }

    let format = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!
    let dump = try AppendedAudioDump(directoryURL: directoryURL)
    let firstBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4)!
    firstBuffer.frameLength = 4
    let firstSamples = firstBuffer.floatChannelData![0]
    firstSamples[0] = 0.25
    firstSamples[1] = -0.5
    firstSamples[2] = 0.75
    firstSamples[3] = -1
    try dump.append(firstBuffer, source: .microphone, generation: 7)

    let secondBuffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 2)!
    secondBuffer.frameLength = 2
    let secondSamples = secondBuffer.floatChannelData![0]
    secondSamples[0] = 0.125
    secondSamples[1] = -0.25
    try dump.append(secondBuffer, source: .microphone, generation: 7)
    dump.close(source: .microphone, generation: 7)

    let url = dump.url(source: .microphone, generation: 7)
    let file = try AVAudioFile(forReading: url)
    assert(file.fileFormat.sampleRate == 48_000)
    assert(file.fileFormat.channelCount == 1)
    assert(file.length == 6)
    let readBuffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 6)!
    try file.read(into: readBuffer)
    assert(readBuffer.frameLength == 6)
    let samples = readBuffer.floatChannelData![0]
    assert(abs(samples[0] - 0.25) < 0.000_001)
    assert(abs(samples[1] + 0.5) < 0.000_001)
    assert(abs(samples[2] - 0.75) < 0.000_001)
    assert(abs(samples[3] + 1) < 0.000_001)
    assert(abs(samples[4] - 0.125) < 0.000_001)
    assert(abs(samples[5] + 0.25) < 0.000_001)
}
