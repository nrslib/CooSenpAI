func testAudioScaling() {
    assert(
        clampedAudioSample(0.5)
            == AudioSampleClampResult(value: 0.5, wasOutOfRange: false)
    )
    assert(
        clampedAudioSample(1.5)
            == AudioSampleClampResult(value: 1, wasOutOfRange: true)
    )
    assert(
        clampedAudioSample(-2)
            == AudioSampleClampResult(value: -1, wasOutOfRange: true)
    )
    assert(
        clampedAudioSample(.nan)
            == AudioSampleClampResult(
                value: 0,
                wasOutOfRange: false,
                wasNonFinite: true
            )
    )
    assert(
        clampedAudioSample(.infinity)
            == AudioSampleClampResult(
                value: 0,
                wasOutOfRange: false,
                wasNonFinite: true
            )
    )
}
