import Foundation

@main
struct HearingHelperApp {
    static func main() {
        let arguments = parseArguments()
        let session = HearingSession(
            locale: arguments.locale,
            inputDevice: arguments.inputDevice,
            sources: arguments.sources,
            debugInputWavPath: arguments.debugInputWavPath,
            debugDumpAppendedPath: arguments.debugDumpAppendedPath,
            debugRequestAuth: arguments.debugRequestAuth
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
