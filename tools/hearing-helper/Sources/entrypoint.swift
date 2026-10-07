import Foundation

@main
struct HearingHelperApp {
    static func main() {
        if runSpeakerOutputDevicesCommandIfRequested() { return }
        if runSpeakerManagementCommandIfRequested() { return }
        if runSpeakerDiagnosisCommandIfRequested() { return }
        let arguments = parseArguments()
        let session = HearingSession(
            locale: arguments.locale,
            engine: arguments.engine,
            inputDevice: arguments.inputDevice,
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
        DispatchQueue.global(qos: .userInitiated).async {
            while let line = readLine() {
                guard let data = line.data(using: .utf8),
                      let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let operation = value["op"] as? String else {
                    continue
                }
                if operation == "cancel" { session.cancel() }
            }
            session.cancel()
        }
        DispatchQueue.main.async { session.authorizeAndStart() }
        RunLoop.main.run()
    }
}
