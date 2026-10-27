import Foundation
import Darwin

private func monitorParent(_ parentPID: pid_t, onGone: @escaping () -> Void) {
    let monitor = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
    monitor.schedule(deadline: .now(), repeating: .milliseconds(250))
    monitor.setEventHandler {
        if getppid() != parentPID {
            monitor.cancel()
            onGone()
        }
    }
    monitor.resume()
}

private func monitorTermination(onSignal: @escaping () -> Void) -> DispatchSourceSignal {
    signal(SIGTERM, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
    source.setEventHandler(handler: onSignal)
    source.resume()
    return source
}

private func parentArguments(_ arguments: [String]) -> (pid: pid_t, withoutParent: [String])? {
    var parentPID: pid_t?
    var remaining: [String] = []
    var index = 0
    while index < arguments.count {
        if arguments[index] == "--parent-pid" {
            guard parentPID == nil, index + 1 < arguments.count,
                  let value = pid_t(arguments[index + 1]), value > 1 else { return nil }
            parentPID = value
            index += 2
        } else {
            remaining.append(arguments[index])
            index += 1
        }
    }
    guard let parentPID else { return nil }
    return (parentPID, remaining)
}

private final class ParentSession {
    private let lock = NSLock()
    private var session: HearingSession?

    func install(_ value: HearingSession) {
        lock.lock()
        session = value
        lock.unlock()
    }

    func parentGone() {
        lock.lock()
        let current = session
        lock.unlock()
        guard let current else { _exit(1) }
        current.cancel()
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + .seconds(5)) {
            _exit(1)
        }
    }
}

@main
struct HearingHelperApp {
    static func main() {
        let startupArguments = Array(CommandLine.arguments.dropFirst())
        guard let parent = parentArguments(startupArguments), getppid() == parent.pid else {
            fputs("hearing helper requires its current --parent-pid\n", stderr)
            exit(2)
        }
        if parent.withoutParent == ["--parent-watch-probe"] {
            let terminationSignal = monitorTermination {
                print("signal-gone")
                fflush(stdout)
                exit(0)
            }
            monitorParent(parent.pid) {
                print("parent-gone")
                fflush(stdout)
                exit(0)
            }
            print("ready")
            fflush(stdout)
            RunLoop.main.run()
            terminationSignal.cancel()
            return
        }
        let parentSession = ParentSession()
        monitorParent(parent.pid) { parentSession.parentGone() }
        if parent.withoutParent.first == "--list-output-devices" {
            _ = runSpeakerOutputDevicesCommandIfRequested()
            return
        }
        if runSpeakerManagementCommandIfRequested(arguments: parent.withoutParent) { return }
        if runSpeakerDiagnosisCommandIfRequested(arguments: parent.withoutParent) { return }
        let arguments = parseArguments(startupArguments)
        let session = HearingSession(
            locale: arguments.locale,
            engine: arguments.engine,
            inputDevice: arguments.inputDevice,
            excludedProcessPIDs: arguments.excludedProcessPIDs,
            parentPID: arguments.parentPID,
            sources: arguments.sources,
            debugInputWavPath: arguments.debugInputWavPath,
            debugDumpAppendedPath: arguments.debugDumpAppendedPath,
            debugRequestAuth: arguments.debugRequestAuth,
            speakerBackend: arguments.speakerBackend,
            speakerDevices: arguments.speakerDevices,
            speakerIdentificationEnabled: arguments.speakerIdentificationEnabled,
            speakerModelPath: arguments.speakerModelPath,
            speakerLedgerPath: arguments.speakerLedgerPath,
            speakerSessionID: arguments.speakerSessionID,
            debugRequestScreenCaptureAuth: arguments.debugRequestScreenCaptureAuth
        )
        parentSession.install(session)
        configureHearingOutputOverload(hearingEventOutput, sources: arguments.sources) { [weak session] source, completion in
            DispatchQueue.main.async {
                guard let session else { completion(); return }
                session.disableSource(
                    source, kind: source.outputOverloadKind,
                    message: "標準出力の取り込みが追いつかず、\(source.rawValue) 入力を停止しました",
                    completion: completion, errorAlreadyEmitted: true
                )
            }
        }
        let terminationSignal = monitorTermination {
            session.cancel()
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + .seconds(5)) {
                _exit(1)
            }
        }
        DispatchQueue.global(qos: .userInitiated).async {
            while let line = readLine() {
                guard let data = line.data(using: .utf8),
                      let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let operation = value["op"] as? String else {
                    continue
                }
                if operation == "cancel" { session.cancel() }
                if operation == "wake-idle", arguments.sources.contains(.microphone),
                   let generation = value["generation"] as? Int, generation > 0 {
                    session.armWakeIdle(generation: generation)
                }
                if operation == "update-exclude-pids", let values = value["pids"] as? [Int],
                   values.allSatisfy({ $0 > 1 && $0 <= Int32.max }) {
                    let pids = values.map(pid_t.init)
                    DispatchQueue.main.async { session.updateExcludedProcessPIDs(pids) }
                }
            }
            session.cancel()
        }
        DispatchQueue.main.async { session.authorizeAndStart() }
        RunLoop.main.run()
        terminationSignal.cancel()
    }
}
