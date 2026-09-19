import Foundation

@main
struct HearingHelperApp {
    static func main() {
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
            speakerIdentificationEnabled: arguments.speakerIdentificationEnabled,
            speakerModelPath: arguments.speakerModelPath,
            speakerLedgerPath: arguments.speakerLedgerPath,
            debugRequestScreenCaptureAuth: arguments.debugRequestScreenCaptureAuth
        )
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
