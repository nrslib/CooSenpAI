import Foundation

// start の失敗は未起動のリソースを解放してから main queue に返す。
// stop は start 成功後だけ呼び、完了も main queue に返す。
protocol SpeakerScreenCaptureDevice: AnyObject {
    func start(completion: @escaping (Error?) -> Void)
    func stop(completion: @escaping (Error?) -> Void)
}

final class SpeakerScreenCapture {
    private enum Phase { case idle, starting, cancellingStart, running, stopping, stopped, failed }
    private let device: SpeakerScreenCaptureDevice
    private let diagnostic: (String) -> Void
    private var phase = Phase.idle
    private var completions: [() -> Void] = []

    init(device: SpeakerScreenCaptureDevice, diagnostic: @escaping (String) -> Void) {
        self.device = device
        self.diagnostic = diagnostic
    }

    func start(onReady: @escaping () -> Void, onFailure: @escaping (Error) -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        precondition(phase == .idle)
        phase = .starting
        device.start { [self] error in
            dispatchPrecondition(condition: .onQueue(.main))
            let cancelled = phase == .cancellingStart
            precondition(phase == .starting || cancelled)
            if let error {
                phase = .failed
                let pending = completions
                completions.removeAll()
                onFailure(error)
                for completion in pending { completion() }
                return
            }
            phase = .running
            if cancelled {
                beginStop()
            } else {
                onReady()
            }
        }
    }

    func stop(completion: @escaping () -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        switch phase {
        case .stopped, .failed: completion()
        case .idle:
            phase = .stopped
            completion()
        case .starting, .cancellingStart:
            completions.append(completion)
            phase = .cancellingStart
        case .running:
            completions.append(completion)
            beginStop()
        case .stopping:
            completions.append(completion)
        }
    }

    private func beginStop() {
        precondition(phase == .running)
        phase = .stopping
        device.stop { [self] error in
            dispatchPrecondition(condition: .onQueue(.main))
            if let error {
                completions.removeAll()
                let details = error as NSError
                diagnostic("screen-capture stop failed: domain=\(details.domain) code=\(details.code) description=\(details.localizedDescription)")
                exit(1)
            }
            phase = .stopped
            diagnostic("speaker-capture backend=screen-capture-kit event=stopped")
            let pending = completions
            completions.removeAll()
            for completion in pending { completion() }
        }
    }
}
