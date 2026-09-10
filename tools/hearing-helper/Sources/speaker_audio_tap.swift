import AVFoundation
import Foundation

final class SpeakerAudioTap: @unchecked Sendable {
    private let worker = DispatchQueue(label: "dev.nrslib.coosenpai.hearing.speaker", qos: .userInitiated)
    private let cancellation = SpeakerAudioCancellation()
    private let makeDevice: () throws -> SpeakerAudioCapture
    private let startupTimeout: TimeInterval
    private let diagnostic: (String) -> Void
    private var device: SpeakerAudioCapture?
    private var timer: DispatchSourceTimer?
    private enum Phase { case opening, running, waitingForDevice, stopped }
    private var phase = Phase.opening
    private var hasStarted = false
    private var onBuffer: ((AVAudioPCMBuffer) -> Void)?

    // Lifecycle completions and deadlines are confined to the main queue.
    private var startupDeadline: DispatchWorkItem?
    private var stopDeadline: DispatchWorkItem?
    private var stopCompletions: [() -> Void] = []
    private var stopFinished = false
    private var onReady: ((AVAudioFormat, Bool) -> Void)?
    private var onInterruption: ((SpeakerAudioTapError) -> Void)?
    private var onFailure: ((SpeakerAudioTapError) -> Void)?

    init(diagnostic: @escaping (String) -> Void, startupTimeout: TimeInterval = 30,
         makeDevice: @escaping () throws -> SpeakerAudioCapture) {
        self.diagnostic = diagnostic
        self.makeDevice = makeDevice
        self.startupTimeout = startupTimeout
    }

    func start(onReady: @escaping (AVAudioFormat, Bool) -> Void,
               onBuffer: @escaping (AVAudioPCMBuffer) -> Void,
               onInterruption: @escaping (SpeakerAudioTapError) -> Void,
               onFailure: @escaping (SpeakerAudioTapError) -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        self.onReady = onReady
        self.onBuffer = onBuffer
        self.onInterruption = onInterruption
        self.onFailure = onFailure
        armStartupDeadline()
        worker.async { [self] in
            guard !cancellation.isCancelled else { return }
            do {
                device = try makeDevice()
                let timer = DispatchSource.makeTimerSource(queue: worker)
                // The real-time callback only copies. This cadence drains the bounded ring off that thread.
                timer.schedule(deadline: .now(), repeating: .milliseconds(5), leeway: .milliseconds(1))
                timer.setEventHandler { [weak self] in self?.drain() }
                self.timer = timer
                timer.resume()
                openDevice()
            } catch { reportFailure(error) }
        }
    }

    func stop(_ completion: @escaping () -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        if stopFinished { completion(); return }
        stopCompletions.append(completion)
        if cancellation.isCancelled { return }
        cancellation.cancel()
        startupDeadline?.cancel()
        let deadline = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.terminateForCleanupFailure("stop-timeout")
        }
        stopDeadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(2), execute: deadline)
        worker.async { [self] in
            phase = .stopped
            timer?.cancel()
            timer = nil
            do {
                try device?.close()
                device = nil
                DispatchQueue.main.async { self.finishStop() }
            } catch {
                DispatchQueue.main.async { self.terminateForCleanupFailure(error.localizedDescription) }
            }
        }
    }

    private func openDevice() {
        guard !cancellation.isCancelled, let device else { return }
        phase = .opening
        do {
            let format = try device.start()
            let reconfigured = hasStarted
            hasStarted = true
            DispatchQueue.main.async { [self] in
                guard !cancellation.isCancelled else { return }
                startupDeadline?.cancel()
                onReady?(format, reconfigured)
                worker.async { [self] in
                    if !cancellation.isCancelled { phase = .running }
                }
            }
        } catch SpeakerAudioTapError.noOutputDevice where hasStarted {
            phase = .waitingForDevice
            DispatchQueue.main.async { [self] in
                guard !cancellation.isCancelled else { return }
                startupDeadline?.cancel()
                onInterruption?(.noOutputDevice)
            }
        } catch { reportFailure(error) }
    }

    private func drain() {
        guard !cancellation.isCancelled, let device,
              phase == .running || phase == .waitingForDevice else { return }
        if device.takeConfigurationChange() {
            reconfigureDevice()
            return
        }
        guard phase == .running else { return }
        do {
            for _ in 0..<32 {
                guard !cancellation.isCancelled, let buffer = try device.nextBuffer() else { break }
                onBuffer?(buffer)
            }
        } catch SpeakerAudioTapError.noOutputDevice {
            reconfigureDevice()
        } catch { reportFailure(error) }
    }

    private func reconfigureDevice() {
        phase = .opening
        DispatchQueue.main.async { [self] in
            guard !cancellation.isCancelled else { return }
            armStartupDeadline()
            worker.async { [self] in
                do {
                    try device?.stop()
                    openDevice()
                } catch { reportFailure(error) }
            }
        }
    }

    private func reportFailure(_ error: Error) {
        phase = .stopped
        timer?.cancel()
        let failure = (error as? SpeakerAudioTapError) ?? .unexpected(error)
        DispatchQueue.main.async { [self] in
            guard !cancellation.isCancelled else { return }
            stop {}
            onFailure?(failure)
        }
    }

    private func armStartupDeadline() {
        startupDeadline?.cancel()
        let deadline = DispatchWorkItem { [weak self] in
            guard let self, !self.cancellation.isCancelled else { return }
            self.stop {}
            self.onFailure?(.startupTimeout)
        }
        startupDeadline = deadline
        // IOProc registration can display the system audio permission prompt.
        // This is a deadline, not a delay before delivering ready.
        DispatchQueue.main.asyncAfter(deadline: .now() + startupTimeout, execute: deadline)
    }

    private func terminateForCleanupFailure(_ reason: String) -> Never {
        // A blocked HAL call cannot be cancelled safely. Never announce release or reuse this process.
        diagnostic("audio-tap-cleanup operation=\(reason) action=terminate-process")
        exit(1)
    }

    private func finishStop() {
        guard !stopFinished else { return }
        stopFinished = true
        stopDeadline?.cancel()
        let completions = stopCompletions
        stopCompletions.removeAll()
        for completion in completions { completion() }
    }
}

private final class SpeakerAudioCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    var isCancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return cancelled
    }
    func cancel() {
        lock.lock()
        cancelled = true
        lock.unlock()
    }
}
