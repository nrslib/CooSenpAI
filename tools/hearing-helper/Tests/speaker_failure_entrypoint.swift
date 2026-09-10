import AVFoundation
import CoreAudio
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
            debugRequestAuth: arguments.debugRequestAuth,
            speakerDeviceFactory: {
                guard #available(macOS 14.2, *) else { throw SpeakerAudioTapError.noOutputDevice }
                return try FailureTestSpeakerDevice()
            }
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

// Only the explicit test build includes this boundary fixture. Microphone capture is real.
@available(macOS 14.2, *)
private final class FailureTestSpeakerDevice: SpeakerAudioCapture {
    private let real: CoreAudioSpeakerDevice
    private let statePath: String
    private var lastState: String?
    init() throws {
        let home = ProcessInfo.processInfo.environment["COOSENPAI_HOME"] ?? NSTemporaryDirectory()
        statePath = ProcessInfo.processInfo.environment["COOSENPAI_SPEAKER_TEST_STATE"]
            ?? (home as NSString).appendingPathComponent("speaker-failure-test")
        lastState = try? String(contentsOfFile: statePath, encoding: .utf8)
        real = try CoreAudioSpeakerDevice(diagnostic: testDiagnostic, onStage: { _, _ in })
    }
    private func checkFailure() throws {
        let state = (try? String(contentsOfFile: statePath, encoding: .utf8))?.trimmingCharacters(in: .whitespacesAndNewlines)
        switch state {
        case "permission":
            testDiagnostic("TEST-INJECTION speaker failure=permission")
            throw SpeakerAudioTapError.operation("test-permission", kAudioDevicePermissionsError)
        case "no-output":
            testDiagnostic("TEST-INJECTION speaker failure=no-output")
            throw SpeakerAudioTapError.noOutputDevice
        default: break
        }
    }
    func start() throws -> AVAudioFormat { try checkFailure(); return try real.start() }
    func takeConfigurationChange() -> Bool {
        let state = try? String(contentsOfFile: statePath, encoding: .utf8)
        let changed = state != lastState
        lastState = state
        return real.takeConfigurationChange() || changed
    }
    func nextBuffer() throws -> AVAudioPCMBuffer? { try checkFailure(); return try real.nextBuffer() }
    func stop() throws { try real.stop() }
    func close() throws {
        try real.close()
        testDiagnostic("TEST-INJECTION speaker cleanup=complete")
    }
}

private func testDiagnostic(_ message: String) {
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}
