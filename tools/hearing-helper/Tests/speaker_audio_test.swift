import AVFoundation
import Foundation

private func awaitSpeakerCondition(_ timeout: TimeInterval = 2, _ condition: () -> Bool) {
    let deadline = Date().addingTimeInterval(timeout)
    while !condition(), Date() < deadline {
        RunLoop.main.run(until: Date().addingTimeInterval(0.005))
    }
    assert(condition(), "Speaker lifecycle condition timed out")
}

private final class TestSpeakerDevice: SpeakerAudioCapture, @unchecked Sendable {
    let lock = NSLock()
    let startGate: DispatchSemaphore?
    var stopFailure = false
    private var change = false
    private var unavailable = false
    private var starts = 0
    private var stops = 0
    private var rate = 48_000.0
    var counts: (Int, Int) { lock.lock(); defer { lock.unlock() }; return (starts, stops) }

    init(blockStart: Bool = false) { startGate = blockStart ? DispatchSemaphore(value: 0) : nil }
    func configure(rate: Double, unavailable: Bool = false) {
        lock.lock(); defer { lock.unlock() }
        self.rate = rate
        self.unavailable = unavailable
        change = true
    }
    func start() throws -> AVAudioFormat {
        lock.lock()
        starts += 1
        let unavailable = self.unavailable
        let rate = self.rate
        lock.unlock()
        startGate?.wait()
        if unavailable { throw SpeakerAudioTapError.noOutputDevice }
        return AVAudioFormat(standardFormatWithSampleRate: rate, channels: 2)!
    }
    func takeConfigurationChange() -> Bool {
        lock.lock(); defer { lock.unlock() }
        defer { change = false }
        return change
    }
    func nextBuffer() throws -> AVAudioPCMBuffer? { nil }
    func stop() throws {
        lock.lock(); stops += 1; lock.unlock()
        if stopFailure { throw SpeakerAudioTapError.operation("stop-device", kAudioHardwareUnspecifiedError) }
    }
}

func testSpeakerAudio() {
    for interleaved in [false, true] {
        let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000,
                                  channels: 2, interleaved: interleaved)!
        var description = format.streamDescription.pointee
        let changes = coosenpai_audio_changes_create()!
        let ring = coosenpai_audio_ring_create(&description, 8, 2, changes)!
        let input = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8)!
        let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8)!
        input.frameLength = 8
        for buffer in UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList) {
            let samples = buffer.mData!.assumingMemoryBound(to: Float.self)
            for i in 0..<Int(buffer.mDataByteSize / 4) { samples[i] = Float(i + 1) / 16 }
        }
        for _ in 0..<20 {
            coosenpai_audio_ring_push(ring, input.audioBufferList)
            output.frameLength = 8
            assert(coosenpai_audio_ring_pop(ring, output.mutableAudioBufferList) == 8)
            for (a, b) in zip(UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList),
                              UnsafeMutableAudioBufferListPointer(output.mutableAudioBufferList)) {
                assert(memcmp(a.mData!, b.mData!, Int(a.mDataByteSize)) == 0)
            }
        }
        for _ in 0..<3 { coosenpai_audio_ring_push(ring, input.audioBufferList) }
        assert(coosenpai_audio_ring_fault(ring) == UInt32(COOSENPAI_AUDIO_RING_OVERFLOW))
        var address = AudioObjectPropertyAddress()
        _ = coosenpai_audio_property_changed(0, 1, &address, UnsafeMutableRawPointer(changes))
        assert(coosenpai_audio_ring_pop(ring, output.mutableAudioBufferList) == 0,
               "A format change must invalidate queued buffers")
        coosenpai_audio_ring_destroy(ring)
        let invalidRing = coosenpai_audio_ring_create(&description, 8, 2, changes)!
        let malformed = input.mutableAudioBufferList
        malformed.pointee.mBuffers.mDataByteSize -= 1
        coosenpai_audio_ring_push(invalidRing, UnsafePointer(malformed))
        assert(coosenpai_audio_ring_fault(invalidRing) == UInt32(COOSENPAI_AUDIO_RING_INVALID_LAYOUT))
        coosenpai_audio_ring_destroy(invalidRing)
        assert(coosenpai_audio_ring_create(&description, UInt32.max, 2, changes) == nil)
        coosenpai_audio_changes_destroy(changes)
    }
    assert(SpeakerAudioTapError.operation("start", kAudioDevicePermissionsError).kind == "system-audio-permission")
    assert(SpeakerAudioTapError.operation("start", kAudioHardwareUnspecifiedError).kind == "system-audio")

    // An OS start that never returns cannot block main-queue cancellation or emit a late ready.
    do {
        let device = TestSpeakerDevice(blockStart: true)
        let tap = SpeakerAudioTap(diagnostic: { _ in }, startupTimeout: 3, makeDevice: { device })
        var ready = false
        var closed = 0
        tap.start(onReady: { _, _ in ready = true }, onBuffer: { _ in },
                  onInterruption: { _ in assertionFailure() }, onFailure: { _ in assertionFailure() })
        awaitSpeakerCondition { device.counts.0 == 1 }
        tap.stop { assert(device.counts.1 == 1); closed += 1 }
        tap.stop { assert(device.counts.1 == 1); closed += 1 }
        assert(closed == 0 && device.counts.1 == 0 && !ready)
        device.startGate!.signal()
        awaitSpeakerCondition { closed == 2 }
        assert(device.counts.1 == 1)
        tap.stop { assert(device.counts.1 == 1); closed += 1 }
        assert(closed == 3 && !ready)
    }
    do {
        let device = TestSpeakerDevice(blockStart: true)
        let tap = SpeakerAudioTap(diagnostic: { _ in }, startupTimeout: 3, makeDevice: { device })
        var failure: String?
        tap.start(onReady: { _, _ in assertionFailure("late ready") }, onBuffer: { _ in },
                  onInterruption: { _ in assertionFailure() }, onFailure: { failure = $0.kind })
        awaitSpeakerCondition(4) { failure != nil }
        assert(failure == "system-audio-start-timeout")
        device.startGate!.signal()
        awaitSpeakerCondition { device.counts.1 == 1 }
        var stopped = false
        tap.stop { stopped = true }
        awaitSpeakerCondition { stopped }
    }
    do {
        let device = TestSpeakerDevice()
        let tap = SpeakerAudioTap(diagnostic: { _ in }, startupTimeout: 3, makeDevice: { device })
        var rates: [Double] = []
        var interruptions = 0
        tap.start(onReady: { format, reconfigured in
            assert(reconfigured == !rates.isEmpty)
            rates.append(format.sampleRate)
        }, onBuffer: { _ in }, onInterruption: {
            assert($0.kind == "system-audio-device"); interruptions += 1
        }, onFailure: { error in assertionFailure(error.localizedDescription) })
        awaitSpeakerCondition { rates.count == 1 }
        device.configure(rate: 16_000)
        awaitSpeakerCondition { rates.count == 2 }
        assert(rates == [48_000, 16_000])
        device.configure(rate: 16_000, unavailable: true)
        awaitSpeakerCondition { interruptions == 1 }
        device.configure(rate: 44_100)
        awaitSpeakerCondition { rates.count == 3 }
        assert(rates.last == 44_100)
        var stopped = false
        tap.stop { stopped = true }
        awaitSpeakerCondition { stopped }
        assert(device.counts == (4, 4))
    }
    do {
        let device = TestSpeakerDevice()
        device.configure(rate: 48_000, unavailable: true)
        let tap = SpeakerAudioTap(diagnostic: { _ in }, startupTimeout: 3, makeDevice: { device })
        var failure: String?
        tap.start(onReady: { _, _ in assertionFailure() }, onBuffer: { _ in },
                  onInterruption: { _ in assertionFailure() }, onFailure: { failure = $0.kind })
        awaitSpeakerCondition { failure != nil }
        assert(failure == "system-audio-device")
        var stopped = false
        tap.stop { stopped = true }
        awaitSpeakerCondition { stopped }
    }
    print("Speaker audio ring/lifecycle tests passed")
}

func runSpeakerStopTimeoutProbe() -> Never {
    let device = TestSpeakerDevice(blockStart: true)
    let tap = SpeakerAudioTap(diagnostic: { message in
        FileHandle.standardError.write(Data("\(message)\n".utf8))
    }, startupTimeout: 30, makeDevice: { device })
    tap.start(onReady: { _, _ in fatalError("ready before start returned") }, onBuffer: { _ in },
              onInterruption: { _ in fatalError("unexpected interruption") },
              onFailure: { _ in fatalError("unexpected startup failure") })
    awaitSpeakerCondition { device.counts.0 == 1 }
    tap.stop { fatalError("completion before release") }
    RunLoop.main.run()
    fatalError("stop timeout did not terminate the process")
}

func runSpeakerStopFailureProbe() -> Never {
    let device = TestSpeakerDevice()
    device.stopFailure = true
    let tap = SpeakerAudioTap(diagnostic: { message in
        FileHandle.standardError.write(Data("\(message)\n".utf8))
    }, makeDevice: { device })
    var ready = false
    tap.start(onReady: { _, _ in ready = true }, onBuffer: { _ in },
              onInterruption: { _ in fatalError("unexpected interruption") },
              onFailure: { _ in fatalError("unexpected startup failure") })
    awaitSpeakerCondition { ready }
    tap.stop { fatalError("completion after stop failure") }
    RunLoop.main.run()
    fatalError("stop failure did not terminate the process")
}
