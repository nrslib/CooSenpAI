import AVFoundation
import Foundation

final class SpeakerAudioTap: @unchecked Sendable {
    private static let cleanupTimeoutExitCode: Int32 = 22
    private static let cleanupFailureExitCode: Int32 = 23
    private static let loadCleanupTimeoutExitCode: Int32 = 24
    private static let loadCleanupFailureExitCode: Int32 = 25
    private enum CleanupFailureKind { case timeout, release }
    private let worker = DispatchQueue(label: "dev.nrslib.coosenpai.hearing.speaker", qos: .userInitiated)
    private let drainQueue = DispatchQueue(label: "dev.nrslib.coosenpai.hearing.speaker.drain", qos: .userInitiated)
    private let cancellation = SpeakerAudioCancellation()
    private let makeDevice: () throws -> SpeakerAudioCapture
    private let startupTimeout: TimeInterval
    private let diagnostic: (String) -> Void
    private var device: SpeakerAudioCapture?
    private var timer: DispatchSourceTimer?
    private enum Phase { case opening, running, waitingForDevice, stopped }
    private var phase = Phase.opening
    private var consuming = false
    private var hasStarted = false
    private var reconfigurationCount = 0
    private var onBuffer: ((SpeakerCapturedBuffer) -> Void)?
    private var onPrepared: ((AVAudioFormat, SpeakerBufferCapacity) throws -> Void)?
    private var onBeforeReopen: ((@escaping () -> Void) -> Void)?
    private var awaitingReady = false
    private var discardedBeforeReady: UInt64 = 0

    // Lifecycle completions and deadlines are confined to the main queue.
    private var startupDeadline: DispatchWorkItem?
    private var stopDeadline: DispatchWorkItem?
    private var stopCompletions: [() -> Void] = []
    private var stopFinished = false
    private var stopForLoadFailure = false
    private var onReady: ((AVAudioFormat, Bool) -> Void)?
    private var onInterruption: ((SpeakerAudioTapError) -> Void)?
    private var onFailure: ((SpeakerAudioTapError) -> Void)?

    init(diagnostic: @escaping (String) -> Void, startupTimeout: TimeInterval = 30,
         makeDevice: @escaping () throws -> SpeakerAudioCapture) {
        self.diagnostic = diagnostic
        self.makeDevice = makeDevice
        self.startupTimeout = startupTimeout
    }

    func start(onPrepared: @escaping (AVAudioFormat, SpeakerBufferCapacity) throws -> Void = { _, _ in },
               onBeforeReopen: @escaping (@escaping () -> Void) -> Void = { completion in completion() },
               onReady: @escaping (AVAudioFormat, Bool) -> Void,
               onBuffer: @escaping (SpeakerCapturedBuffer) -> Void,
               onInterruption: @escaping (SpeakerAudioTapError) -> Void,
               onFailure: @escaping (SpeakerAudioTapError) -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        self.onPrepared = onPrepared
        self.onBeforeReopen = onBeforeReopen
        self.onReady = onReady
        self.onBuffer = onBuffer
        self.onInterruption = onInterruption
        self.onFailure = onFailure
        armStartupDeadline()
        worker.async { [self] in
            guard !cancellation.isCancelled else { return }
            do {
                device = try makeDevice()
                let timer = DispatchSource.makeTimerSource(queue: drainQueue)
                // The real-time callback only copies. This cadence drains the bounded ring off that thread.
                timer.schedule(deadline: .now(), repeating: .milliseconds(5), leeway: .milliseconds(1))
                timer.setEventHandler { [weak self] in self?.drain() }
                drainQueue.sync { self.timer = timer }
                timer.resume()
                openDevice()
            } catch { reportFailure(error) }
        }
    }

    func updateExcludedProcessPIDs(_ pids: [pid_t]) {
        dispatchPrecondition(condition: .onQueue(.main))
        worker.async { [self] in
            device?.updateExcludedProcessPIDs(pids)
        }
    }

    func stop(forLoadFailure: Bool = false, _ completion: @escaping () -> Void) {
        dispatchPrecondition(condition: .onQueue(.main))
        stopForLoadFailure = stopForLoadFailure || forLoadFailure
        if stopFinished { completion(); return }
        stopCompletions.append(completion)
        if cancellation.isCancelled { return }
        cancellation.cancel()
        startupDeadline?.cancel()
        let deadline = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.terminateForCleanupFailure("stop-timeout", kind: .timeout)
        }
        stopDeadline = deadline
        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(2), execute: deadline)
        worker.async { [self] in
            drainQueue.sync {
                phase = .stopped
                consuming = false
                timer?.cancel()
                timer = nil
            }
            do {
                try device?.close()
                device = nil
                DispatchQueue.main.async { self.finishStop() }
            } catch {
                DispatchQueue.main.async {
                    self.terminateForCleanupFailure(error.localizedDescription, kind: .release)
                }
            }
        }
    }

    private func openDevice() {
        guard !cancellation.isCancelled, let device else { return }
        guard let onPrepared else { preconditionFailure("speaker capture preparation is missing") }
        drainQueue.sync {
            phase = .opening
            consuming = false
        }
        do {
            let format = try device.start(onPrepared: onPrepared, onCapturing: { [self] in
                drainQueue.sync {
                    awaitingReady = true
                    discardedBeforeReady = 0
                    consuming = true
                }
            })
            let reconfigured = hasStarted
            hasStarted = true
            drainQueue.sync { phase = .running }
            DispatchQueue.main.async { [self] in
                guard !cancellation.isCancelled else { return }
                startupDeadline?.cancel()
                onReady?(format, reconfigured)
                if device.missingDeviceCount > 0 {
                    onInterruption?(.speakerDevicesMissing(device.missingDeviceCount))
                }
                drainQueue.async { [self] in
                    guard !cancellation.isCancelled else { return }
                    emitSpeakerLoadDiagnostic(
                        "speaker-capture-startup-discarded buffers=\(discardedBeforeReady)", via: diagnostic
                    )
                    awaitingReady = false
                }
            }
        } catch let error as SpeakerAudioTapError where error.isRecoverableDeviceInterruption(hasStarted: hasStarted) {
            if case .speakerDevicesMissing = error { hasStarted = true }
            if case .outputDeviceListUnavailable = error { hasStarted = true }
            drainQueue.sync {
                consuming = false
                phase = .waitingForDevice
            }
            DispatchQueue.main.async { [self] in
                guard !cancellation.isCancelled else { return }
                startupDeadline?.cancel()
                onInterruption?(error)
            }
        } catch { reportFailure(error) }
    }

    private func drain() {
        guard !cancellation.isCancelled,
              phase == .running || phase == .waitingForDevice || consuming,
              let device else { return }
        do {
            if phase != .opening, let reason = try device.takeConfigurationChange() {
                reconfigureDevice(reason: reason)
                return
            }
            guard consuming else { return }
            for _ in 0..<32 {
                guard !cancellation.isCancelled,
                      let buffer = try device.nextBuffer(checkDelivery: phase == .running) else { break }
                if awaitingReady {
                    discardedBeforeReady += 1
                } else {
                    onBuffer?(buffer)
                }
            }
        } catch SpeakerAudioTapError.noOutputDevice {
            reconfigureDevice(reason: "output-device-missing")
        } catch SpeakerAudioTapError.speakerDevicesMissing {
            reconfigureDevice(reason: "selected-output-device-missing")
        } catch { reportFailure(error) }
    }

    private func reconfigureDevice(reason: String) {
        reconfigurationCount += 1
        emitSpeakerLoadDiagnostic("audio-tap-reconfigure count=\(reconfigurationCount) reason=\(reason)",
                                  via: diagnostic)
        phase = .opening
        consuming = false
        DispatchQueue.main.async { [self] in
            guard !cancellation.isCancelled else { return }
            onInterruption?(.reconfiguring)
            armStartupDeadline()
            worker.async { [self] in
                do {
                    try device?.stop()
                    guard let onBeforeReopen else { preconditionFailure("reopen preparation is missing") }
                    onBeforeReopen { [self] in
                        worker.async { [self] in
                            guard !cancellation.isCancelled else { return }
                            openDevice()
                        }
                    }
                } catch { reportFailure(error) }
            }
        }
    }

    private func reportFailure(_ error: Error) {
        let failure = (error as? SpeakerAudioTapError) ?? .unexpected(error)
        drainQueue.async { [self] in
            phase = .stopped
            consuming = false
            timer?.cancel()
            DispatchQueue.main.async { [self] in
                guard !cancellation.isCancelled else { return }
                // The owner publishes load failures before requesting HAL teardown.
                onFailure?(failure)
            }
        }
    }

    private func armStartupDeadline() {
        startupDeadline?.cancel()
        let deadline = DispatchWorkItem { [weak self] in
            guard let self, !self.cancellation.isCancelled else { return }
            self.stop(forLoadFailure: false) {}
            self.onFailure?(.startupTimeout)
        }
        startupDeadline = deadline
        // IOProc registration can display the system audio permission prompt.
        // This is a deadline, not a delay before delivering ready.
        DispatchQueue.main.asyncAfter(deadline: .now() + startupTimeout, execute: deadline)
    }

    private func terminateForCleanupFailure(_ reason: String, kind: CleanupFailureKind) -> Never {
        // A blocked HAL call cannot be cancelled safely. Never announce release or reuse this process.
        diagnostic("audio-tap-cleanup operation=\(reason) action=terminate-process")
        let exitCode: Int32
        switch (kind, stopForLoadFailure) {
        case (.timeout, false): exitCode = Self.cleanupTimeoutExitCode
        case (.release, false): exitCode = Self.cleanupFailureExitCode
        case (.timeout, true): exitCode = Self.loadCleanupTimeoutExitCode
        case (.release, true): exitCode = Self.loadCleanupFailureExitCode
        }
        exit(exitCode)
    }

    private func finishStop() {
        guard !stopFinished else { return }
        stopFinished = true
        diagnostic("speaker-capture backend=process-tap event=stopped")
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
