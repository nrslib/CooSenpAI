import Foundation

// Lifecycle methods run on the supplied serial queue; audio callbacks only read the token.
final class MicrophoneInputGeneration: @unchecked Sendable {
    private let lock = NSLock()
    private var valid = true

    var isValid: Bool {
        lock.lock()
        defer { lock.unlock() }
        return valid
    }

    fileprivate func invalidate() {
        lock.lock()
        valid = false
        lock.unlock()
    }
}

final class MicrophoneInputRecovery {
    private let queue: DispatchQueue
    private let delay: DispatchTimeInterval
    private let restart: () -> Void
    private var generation: MicrophoneInputGeneration?
    private var pending: DispatchWorkItem?

    init(queue: DispatchQueue = .main, delay: DispatchTimeInterval = .milliseconds(100),
         restart: @escaping () -> Void) {
        self.queue = queue
        self.delay = delay
        self.restart = restart
    }

    func begin() -> MicrophoneInputGeneration {
        dispatchPrecondition(condition: .onQueue(queue))
        stop()
        let token = MicrophoneInputGeneration()
        generation = token
        return token
    }

    func requestRestart(for token: MicrophoneInputGeneration) {
        dispatchPrecondition(condition: .onQueue(queue))
        guard generation === token, token.isValid, pending == nil else { return }
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.generation === token, token.isValid else { return }
            self.pending = nil
            self.restart()
        }
        pending = work
        queue.asyncAfter(deadline: .now() + delay, execute: work)
    }

    func stop() {
        dispatchPrecondition(condition: .onQueue(queue))
        generation?.invalidate()
        generation = nil
        pending?.cancel()
        pending = nil
    }
}
