private struct FakeRecognitionRequest {
    private(set) var endAudioCount = 0

    mutating func endAudio() {
        endAudioCount += 1
    }
}

private final class FakeRecognitionTask {
    private(set) var cancelCount = 0

    func cancel() {
        cancelCount += 1
    }
}

func testRecognitionState() {
    var callbackValues: [Int] = []
    let callbackGate = RecognitionCallbackGate()
    callbackGate.enqueue { callbackValues.append(1) }
    assert(callbackValues.isEmpty)
    callbackGate.open()
    assert(callbackValues == [1])
    callbackGate.enqueue { callbackValues.append(2) }
    assert(callbackValues == [1, 2])
    let discardedCallbackGate = RecognitionCallbackGate()
    discardedCallbackGate.enqueue { callbackValues.append(3) }
    discardedCallbackGate.discard()
    discardedCallbackGate.open()
    assert(callbackValues == [1, 2])

    var states = RecognitionStateStore<String, String, String>()
    let firstGeneration = states.reserveGeneration(for: .microphone)
    assert(firstGeneration == 1)
    assert(
        states.install(
            source: .microphone,
            request: "request-1",
            task: "task-1",
            recognizer: "recognizer-1",
            generation: firstGeneration,
            sourceIsActive: true
        )
    )
    assert(states.currentRequest(for: .microphone) == "request-1")

    let secondGeneration = states.reserveGeneration(for: .microphone)
    assert(secondGeneration == 2)
    assert(
        states.install(
            source: .microphone,
            request: "stale-request",
            task: "stale-task",
            recognizer: "stale-recognizer",
            generation: firstGeneration,
            sourceIsActive: true
        ) == false
    )
    assert(states.currentRequest(for: .microphone) == "request-1")
    assert(!states.isCurrentGeneration(.microphone, firstGeneration))
    assert(states.isCurrentGeneration(.microphone, secondGeneration))
    assert(!states.isCurrentState(.microphone, firstGeneration))

    let replacementRejected = states.install(
        source: .microphone,
        request: "request-2",
        task: "task-2",
        recognizer: "recognizer-2",
        generation: secondGeneration,
        sourceIsActive: true
    )
    assert(!replacementRejected)
    assert(states.currentRequest(for: .microphone) == "request-1")
    assert(states.currentGeneration(for: .microphone) == firstGeneration)

    let removed = states.remove(source: .microphone, generation: firstGeneration)
    assert(removed?.request == "request-1")
    assert(
        states.install(
            source: .microphone,
            request: "request-2",
            task: "task-2",
            recognizer: "recognizer-2",
            generation: secondGeneration,
            sourceIsActive: true
        )
    )
    assert(states.currentRequest(for: .microphone) == "request-2")
    assert(states.currentGeneration(for: .microphone) == secondGeneration)

    let removedSecond = states.remove(source: .microphone, generation: secondGeneration)
    assert(removedSecond?.request == "request-2")
    assert(states.currentRequest(for: .microphone) == nil)
    assert(!states.isCurrentState(.microphone, secondGeneration))

    var inactiveAvailability = AudioSourceAvailability(sources: [.speaker])
    assert(inactiveAvailability.disable(.speaker))
    var inactiveStates = RecognitionStateStore<String, String, String>()
    let inactiveGeneration = inactiveStates.reserveGeneration(for: .speaker)
    assert(
        inactiveStates.install(
            source: .speaker,
            request: "inactive-request",
            task: "inactive-task",
            recognizer: "inactive-recognizer",
            generation: inactiveGeneration,
            sourceIsActive: inactiveAvailability.isActive(.speaker)
        ) == false
    )
    assert(inactiveStates.currentRequest(for: .speaker) == nil)

    var lifecycleStates = RecognitionStateStore<String, String, String>()
    let lifecycleGeneration = lifecycleStates.reserveGeneration(for: .microphone)
    assert(
        lifecycleStates.install(
            source: .microphone,
            request: "lifecycle-request",
            task: "lifecycle-task",
            recognizer: "lifecycle-recognizer",
            generation: lifecycleGeneration,
            sourceIsActive: true
        )
    )
    assert(lifecycleStates.lifecycle(for: .microphone) == .accepting)
    assert(lifecycleStates.acceptsAudio(for: .microphone))
    assert(
        lifecycleStates.beginEnding(
            source: .microphone,
            generation: lifecycleGeneration,
            reason: .trailing
        )?.lifecycle == .ending
    )
    assert(lifecycleStates.lifecycle(for: .microphone) == .ending)
    assert(lifecycleStates.closeReason(for: .microphone) == .trailing)
    assert(!lifecycleStates.acceptsAudio(for: .microphone))
    assert(
        lifecycleStates.markTaskTerminal(
            source: .microphone,
            generation: lifecycleGeneration
        )
    )
    assert(lifecycleStates.lifecycle(for: .microphone) == .ending)
    assert(lifecycleStates.taskTerminalArrived(for: .microphone))
    assert(
        lifecycleStates.beginEnding(
            source: .microphone,
            generation: lifecycleGeneration
        ) == nil
    )
    assert(
        lifecycleStates.beginCancelling(
            source: .microphone,
            generation: lifecycleGeneration
        ) == nil
    )
    assert(lifecycleStates.lifecycle(for: .microphone) == .ending)
    assert(
        !lifecycleStates.markTaskTerminal(
            source: .microphone,
            generation: lifecycleGeneration
        )
    )
    let endedState = lifecycleStates.remove(
        source: .microphone,
        generation: lifecycleGeneration
    )
    assert(endedState?.lifecycle == .ending)
    assert(endedState?.closeReason == .trailing)

    var finalBeforeVadClose = RecognitionStateStore<String, String, String>()
    let finalGeneration = finalBeforeVadClose.reserveGeneration(for: .microphone)
    assert(
        finalBeforeVadClose.install(
            source: .microphone,
            request: "final-request",
            task: "final-task",
            recognizer: "recognizer",
            generation: finalGeneration,
            sourceIsActive: true
        )
    )
    assert(
        finalBeforeVadClose.markTaskTerminal(
            source: .microphone,
            generation: finalGeneration
        )
    )
    assert(finalBeforeVadClose.lifecycle(for: .microphone) == .terminal)
    assert(
        finalBeforeVadClose.beginEnding(
            source: .microphone,
            generation: finalGeneration,
            reason: .trailing
        ) == nil
    )
    let finalState = finalBeforeVadClose.remove(
        source: .microphone,
        generation: finalGeneration
    )
    assert(finalState?.lifecycle == .terminal)
    assert(
        shouldRearmRecognitionImmediately(
            lifecycle: finalState?.lifecycle ?? .terminal,
            vadWasSpeaking: true,
            closeReason: finalState?.closeReason
        )
    )

    let cancellingGeneration = lifecycleStates.reserveGeneration(for: .microphone)
    assert(
        lifecycleStates.install(
            source: .microphone,
            request: "cancelling-request",
            task: "cancelling-task",
            recognizer: "cancelling-recognizer",
            generation: cancellingGeneration,
            sourceIsActive: true
        )
    )
    assert(
        lifecycleStates.beginEnding(
            source: .microphone,
            generation: cancellingGeneration
        )?.lifecycle == .ending
    )
    assert(
        lifecycleStates.beginCancelling(
            source: .microphone,
            generation: cancellingGeneration
        )?.lifecycle == .cancelling
    )
    assert(lifecycleStates.currentGeneration(for: .microphone) == cancellingGeneration)
    assert(!lifecycleStates.taskTerminalArrived(for: .microphone))
    assert(lifecycleStates.taskCancellationRequested(for: .microphone))
    assert(
        lifecycleStates.markTaskTerminal(
            source: .microphone,
            generation: cancellingGeneration
        )
    )
    assert(lifecycleStates.lifecycle(for: .microphone) == .cancelling)
    assert(lifecycleStates.currentGeneration(for: .microphone) == cancellingGeneration)
    assert(
        lifecycleStates.remove(
            source: .microphone,
            generation: cancellingGeneration
        )?.lifecycle == .cancelling
    )

    var terminalStates = RecognitionStateStore<String, String, String>()
    let terminalGeneration = terminalStates.reserveGeneration(for: .speaker)
    assert(
        terminalStates.markTaskTerminal(
            source: .speaker,
            generation: terminalGeneration
        )
    )
    assert(
        terminalStates.install(
            source: .speaker,
            request: "terminal-request",
            task: "terminal-task",
            recognizer: "terminal-recognizer",
            generation: terminalGeneration,
            sourceIsActive: true
        )
    )
    assert(terminalStates.lifecycle(for: .speaker) == .terminal)
    assert(!terminalStates.acceptsAudio(for: .speaker))
    assert(
        !terminalStates.markTaskTerminal(
            source: .speaker,
            generation: terminalGeneration
        )
    )
    assert(
        terminalStates.remove(
            source: .speaker,
            generation: terminalGeneration
        )?.lifecycle == .terminal
    )
    assert(
        !terminalStates.markTaskTerminal(
            source: .speaker,
            generation: terminalGeneration
        )
    )

    assert(RecognitionRestartThrottle.minimumIntervalNanoseconds == 2_000_000_000)

    assert(
        shouldRearmRecognitionImmediately(
            lifecycle: .accepting,
            vadWasSpeaking: false,
            closeReason: nil
        )
    )
    assert(
        shouldRearmRecognitionImmediately(
            lifecycle: .ending,
            vadWasSpeaking: true,
            closeReason: .max
        )
    )
    assert(
        shouldRearmRecognitionImmediately(
            lifecycle: .ending,
            vadWasSpeaking: false,
            closeReason: .trailing
        )
    )
    assert(
        !shouldRearmRecognitionImmediately(
            lifecycle: .ending,
            vadWasSpeaking: false,
            closeReason: .max
        )
    )
    assert(
        !shouldRearmRecognitionImmediately(
            lifecycle: .ending,
            vadWasSpeaking: false,
            closeReason: .steadyNoise
        )
    )

    let cancellationDeadline = RecognitionCancellationDeadline(
        startedAt: 10_000,
        timeoutNanoseconds: 1_000
    )
    assert(cancellationDeadline.deadlineNanoseconds == 11_000)
    assert(!cancellationDeadline.isExpired(at: 10_999))
    assert(cancellationDeadline.isExpired(at: 11_000))
    assert(cancellationDeadline.delayNanoseconds(from: 10_500) == 500)
    assert(cancellationDeadline.delayNanoseconds(from: 11_000) == 0)

    var tracker = RecognitionRestartTracker()
    for second in 0..<30 {
        assert(!tracker.recordRestart(at: UInt64(second) * 1_000_000_000))
    }
    assert(tracker.recentRestartCount == 30)
    assert(tracker.recordRestart(at: 30_000_000_000))
    assert(tracker.recentRestartCount == 31)
    assert(!tracker.recordRestart(at: 61_000_000_000))
    assert(tracker.recentRestartCount == 30)

    var pendingAudio = PendingAudioWindow<Int>(capacityNanoseconds: 5_000_000_000)
    assert(pendingAudio.append(1, durationNanoseconds: 1_000_000_000))
    var coordinator = RecognitionPendingCoordinator()
    assert(
        coordinator.finishSegment(
            hasPendingAudio: !pendingAudio.isEmpty,
            cooldownNanoseconds: 2_000_000_000,
            at: 10_000_000_000
        ) == .drainAfter(12_000_000_000)
    )
    assert(!pendingAudio.isEmpty)
    assert(pendingAudio.append(2, durationNanoseconds: 1_000_000_000))
    assert(coordinator.isBlocked(at: 11_999_999_999))
    assert(!coordinator.isBlocked(at: 12_000_000_000))
    coordinator.markReady()
    assert(!coordinator.isBlocked(at: 11_000_000_000))

    var emptyCoordinator = RecognitionPendingCoordinator()
    assert(
        emptyCoordinator.finishSegment(
            hasPendingAudio: false,
            cooldownNanoseconds: 2_000_000_000,
            at: 10_000_000_000
        ) == .drainAfter(12_000_000_000)
    )
    assert(emptyCoordinator.hasPendingCooldown)
    assert(emptyCoordinator.isBlocked(at: 11_999_999_999))
    assert(!emptyCoordinator.isBlocked(at: 12_000_000_000))

    var boundedPending = PendingAudioWindow<Int>(capacityNanoseconds: 5_000)
    for value in 1...5 {
        assert(
            boundedPending.appendKeepingLatest(
                value,
                durationNanoseconds: 1_000
            ) == .appended
        )
    }
    assert(
        boundedPending.appendKeepingLatest(
            6,
            durationNanoseconds: 1_000
        ) == .droppedOldest(count: 1)
    )
    assert(boundedPending.removeAll() == [2, 3, 4, 5, 6])

    var pendingWhileCooldown = PendingAudioWindow<Int>(capacityNanoseconds: 5_000)
    var cooldownCoordinator = RecognitionPendingCoordinator()
    assert(
        cooldownCoordinator.finishSegment(
            hasPendingAudio: false,
            cooldownNanoseconds: 2_000,
            at: 0
        ) == .drainAfter(2_000)
    )
    for value in 1...7 {
        let result = pendingWhileCooldown.appendKeepingLatest(
            value,
            durationNanoseconds: 1_000
        )
        assert(result != .rejected)
    }
    assert(cooldownCoordinator.isBlocked(at: 1_999))
    assert(!cooldownCoordinator.isBlocked(at: 2_000))
    assert(pendingWhileCooldown.removeAll() == [3, 4, 5, 6, 7])

    var pendingDuringRecognitionWait = PendingAudioWindow<Int>(
        capacityNanoseconds: pendingAudioWindowCapacityNanoseconds
    )
    let waitDuration = recognitionFinalTimeoutNanoseconds
        + RecognitionRestartThrottle.minimumIntervalNanoseconds
        + recognitionCancellationTimeoutNanoseconds
    for value in 0...Int(waitDuration / 1_000_000_000) {
        assert(
            pendingDuringRecognitionWait.appendKeepingLatest(
                value,
                durationNanoseconds: 1_000_000_000
            ) != .rejected
        )
    }
    assert(!pendingDuringRecognitionWait.isEmpty)
    assert(
        pendingDuringRecognitionWait.appendKeepingLatest(
            99,
            durationNanoseconds: 1_000_000_000
        ) == .droppedOldest(count: 1)
    )

    var availability = AudioSourceAvailability(sources: [.microphone, .speaker])
    assert(availability.disable(.speaker))
    assert(availability.isActive(.microphone))
    assert(availability.hasActiveSource)
    assert(!availability.disable(.speaker))
    assert(availability.disable(.microphone))
    assert(!availability.hasActiveSource)

    testRecognitionTerminalOrderingAndSideEffects()
    testRecognitionCancellationTimeoutRecovery()
}

private func testRecognitionTerminalOrderingAndSideEffects() {
    var states = RecognitionStateStore<String, String, String>()
    let generation = states.reserveGeneration(for: .microphone)
    assert(
        states.install(
            source: .microphone,
            request: "request",
            task: "task",
            recognizer: "recognizer",
            generation: generation,
            sourceIsActive: true
        )
    )

    var request = FakeRecognitionRequest()
    let task = FakeRecognitionTask()
    if states.beginEnding(source: .microphone, generation: generation) != nil {
        request.endAudio()
    }
    if states.beginEnding(source: .microphone, generation: generation) != nil {
        request.endAudio()
    }
    assert(request.endAudioCount == 1)

    assert(states.markTaskTerminal(source: .microphone, generation: generation))
    assert(
        states.beginCancelling(source: .microphone, generation: generation) == nil
    )
    assert(task.cancelCount == 0)
    assert(
        states.remove(source: .microphone, generation: generation)?.lifecycle == .ending
    )

    let cancellingGeneration = states.reserveGeneration(for: .microphone)
    assert(
        states.install(
            source: .microphone,
            request: "cancelling-request",
            task: "cancelling-task",
            recognizer: "recognizer",
            generation: cancellingGeneration,
            sourceIsActive: true
        )
    )
    assert(
        states.beginEnding(
            source: .microphone,
            generation: cancellingGeneration
        ) != nil
    )
    if states.beginCancelling(
        source: .microphone,
        generation: cancellingGeneration
    ) != nil {
        task.cancel()
    }
    if states.beginCancelling(
        source: .microphone,
        generation: cancellingGeneration
    ) != nil {
        task.cancel()
    }
    assert(task.cancelCount == 1)
}

private func testRecognitionCancellationTimeoutRecovery() {
    var states = RecognitionStateStore<String, FakeRecognitionTask, String>()
    let generation = states.reserveGeneration(for: .microphone)
    let task = FakeRecognitionTask()
    assert(
        states.install(
            source: .microphone,
            request: "request",
            task: task,
            recognizer: "recognizer",
            generation: generation,
            sourceIsActive: true
        )
    )
    assert(states.beginEnding(source: .microphone, generation: generation) != nil)
    let cancellingState = states.beginCancelling(
        source: .microphone,
        generation: generation
    )
    assert(cancellingState != nil)
    assert(!states.taskTerminalArrived(for: .microphone))
    task.cancel()
    assert(task.cancelCount == 1)

    let recovered = states.recoverCancellationTimeout(
        source: .microphone,
        generation: generation
    )
    assert(recovered?.taskCancellationRequested == true)
    assert(recovered?.task.cancelCount == 1)
    assert(states.currentGeneration(for: .microphone) == nil)
    assert(
        !states.markTaskTerminal(
            source: .microphone,
            generation: generation
        )
    )

    let nextGeneration = states.reserveGeneration(for: .microphone)
    assert(nextGeneration == generation + 1)

    var tracker = RecognitionCancellationTimeoutRecoveryTracker()
    assert(!tracker.recordRecovery())
    assert(!tracker.recordRecovery())
    assert(tracker.recordRecovery())
    tracker.reset()
    assert(!tracker.recordRecovery())
}
