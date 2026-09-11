import Foundation

private final class DeferredScreenCaptureDevice: SpeakerScreenCaptureDevice {
    var startCompleted: ((Error?) -> Void)?
    var stopCompleted: ((Error?) -> Void)?
    var stopCount = 0
    var stopError: Error?

    func start(completion: @escaping (Error?) -> Void) { startCompleted = completion }
    func stop(completion: @escaping (Error?) -> Void) {
        stopCount += 1
        if let stopError { completion(stopError); return }
        stopCompleted = completion
    }
}

func testSpeakerScreenCapture() {
    for startFails in [false, true] {
        for cancelDuringStart in [false, true] {
            let device = DeferredScreenCaptureDevice()
            let startError = NSError(domain: "test-start", code: 1)
            if startFails { device.stopError = NSError(domain: "test-stop", code: 2) }
            var events: [String] = []
            let capture = SpeakerScreenCapture(device: device, diagnostic: { _ in events.append("stopped") })
            capture.start(onReady: { events.append("ready") }, onFailure: { error in
                assert(error as NSError === startError, "元の起動エラーをそのまま通知する")
                events.append("start-error")
                capture.stop { events.append("failure-completion") }
            })
            if cancelDuringStart {
                capture.stop { events.append("closed") }
                capture.stop { events.append("second-completion") }
                assert(device.stopCount == 0 && events.isEmpty, "起動と停止を並行実行しない")
            }
            device.startCompleted?(startFails ? startError : nil)
            if startFails {
                let expected = cancelDuringStart
                    ? ["start-error", "failure-completion", "closed", "second-completion"]
                    : ["start-error", "failure-completion"]
                assert(events == expected, "起動失敗は停止エラーに置き換えず、取消完了より先に通知する")
                capture.stop { events.append("after-failure") }
                assert(device.stopCount == 0 && events.last == "after-failure", "起動失敗後は停止を呼ばない")
                continue
            }
            if !cancelDuringStart {
                assert(events == ["ready"])
                capture.stop { events.append("closed") }
            }
            assert(device.stopCount == 1, "起動成功後の停止は一度だけ")
            assert(!events.contains("closed") && !events.contains("start-error"), "停止完了前に終端を返さない")
            device.stopCompleted?(nil)
            if cancelDuringStart {
                assert(events == ["stopped", "closed", "second-completion"])
            } else {
                assert(events == ["ready", "stopped", "closed"])
            }
            capture.stop { events.append("after-stop") }
            assert(device.stopCount == 1 && events.last == "after-stop")
        }
    }
    print("ScreenCaptureKit startup/cancel lifecycle tests passed")
    print("ScreenCaptureKit start failure: original error preserved without stop (including cancel)")
}

func testSpeakerScreenCaptureStopFailure() {
    let device = DeferredScreenCaptureDevice()
    let capture = SpeakerScreenCapture(device: device, diagnostic: {
        FileHandle.standardError.write(Data("\($0)\n".utf8))
    })
    capture.start(onReady: { fatalError("cancel 後に ready を返さない") },
                  onFailure: { _ in fatalError("停止失敗はプロセスを終了する") })
    capture.stop { FileHandle.standardOutput.write(Data("closed\n".utf8)) }
    device.startCompleted?(nil)
    device.stopCompleted?(NSError(domain: "test-stop", code: 1))
    fatalError("停止失敗後は戻らない")
}
