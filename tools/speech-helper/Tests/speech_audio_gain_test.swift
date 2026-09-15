import AVFoundation

func testSpeechAudioGain() {
    let floatFormat = AVAudioFormat(
        commonFormat: .pcmFormatFloat32,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!
    let quiet = AVAudioPCMBuffer(pcmFormat: floatFormat, frameCapacity: 2)!
    quiet.frameLength = 2
    quiet.floatChannelData![0][0] = 0.001
    quiet.floatChannelData![0][1] = -0.002
    let quietResult = try! SpeechAudioGain.apply(to: quiet)
    assert(abs(quietResult.inputPeak - 0.002) < 0.000_001)
    assert(quietResult.gain == SpeechAudioGain.maximumGain)
    assert(abs(Double(quiet.floatChannelData![0][1]) + 0.064) < 0.000_001)

    let loud = AVAudioPCMBuffer(pcmFormat: floatFormat, frameCapacity: 1)!
    loud.frameLength = 1
    loud.floatChannelData![0][0] = 0.2
    let loudResult = try! SpeechAudioGain.apply(to: loud)
    assert(loudResult.gain == 1)
    assert(loud.floatChannelData![0][0] == 0.2)

    let int16Format = AVAudioFormat(
        commonFormat: .pcmFormatInt16,
        sampleRate: 48_000,
        channels: 1,
        interleaved: false
    )!
    let int16 = AVAudioPCMBuffer(pcmFormat: int16Format, frameCapacity: 2)!
    int16.frameLength = 2
    int16.int16ChannelData![0][0] = 33
    int16.int16ChannelData![0][1] = -66
    let int16Result = try! SpeechAudioGain.apply(to: int16)
    assert(int16Result.gain == SpeechAudioGain.maximumGain)
    assert(int16.int16ChannelData![0][0] == 1_056)
    assert(int16.int16ChannelData![0][1] == -2_112)
}
