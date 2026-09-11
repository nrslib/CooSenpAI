import Foundation
@preconcurrency import ScreenCaptureKit

final class ScreenCaptureSpeakerDevice: SpeakerScreenCaptureDevice, @unchecked Sendable {
    private weak var output: (SCStreamOutput & SCStreamDelegate)?
    private let stage: (String, String) -> Void
    private let diagnostic: (String) -> Void
    private var stream: SCStream?

    init(output: SCStreamOutput & SCStreamDelegate, stage: @escaping (String, String) -> Void,
         diagnostic: @escaping (String) -> Void) {
        self.output = output
        self.stage = stage
        self.diagnostic = diagnostic
    }

    func start(completion: @escaping (Error?) -> Void) {
        Task {
            var registeredOutput: SCStreamOutput?
            do {
                stage("shareable-content", "begin")
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                stage("shareable-content", "end")
                guard let display = content.displays.first else { throw CaptureError.noDisplay }
                guard let output else { throw CaptureError.outputReleased }
                stage("stream-create", "begin")
                let filter = SCContentFilter(display: display, excludingWindows: [])
                let configuration = SCStreamConfiguration()
                configuration.capturesAudio = true
                configuration.excludesCurrentProcessAudio = true
                configuration.sampleRate = 48_000
                configuration.channelCount = 2
                diagnostic("audio-format speaker capture=sampleRate=\(configuration.sampleRate) channels=\(configuration.channelCount) commonFormat=unknown-until-first-sample converted-append=mono-float32")
                let stream = SCStream(filter: filter, configuration: configuration, delegate: output)
                self.stream = stream
                stage("stream-create", "end")
                stage("add-output", "begin")
                try stream.addStreamOutput(output, type: .audio,
                    sampleHandlerQueue: DispatchQueue(label: "dev.nrslib.coosenpai.hearing.audio"))
                registeredOutput = output
                stage("add-output", "end")
                stage("start-capture", "begin")
                try await stream.startCapture()
                stage("start-capture", "end")
                DispatchQueue.main.async { completion(nil) }
            } catch let startError {
                if let stream, let registeredOutput {
                    do {
                        try stream.removeStreamOutput(registeredOutput, type: .audio)
                    } catch {
                        let details = error as NSError
                        diagnostic("screen-capture remove-output failed: domain=\(details.domain) code=\(details.code) description=\(details.localizedDescription)")
                    }
                }
                self.stream = nil
                registeredOutput = nil
                DispatchQueue.main.async { completion(startError) }
            }
        }
    }

    func stop(completion: @escaping (Error?) -> Void) {
        guard let stream else { completion(nil); return }
        Task {
            do {
                try await stream.stopCapture()
                self.stream = nil
                DispatchQueue.main.async { completion(nil) }
            } catch {
                DispatchQueue.main.async { completion(error) }
            }
        }
    }

    private enum CaptureError: LocalizedError {
        case noDisplay, outputReleased
        var errorDescription: String? {
            switch self {
            case .noDisplay: return "音声を取得できるディスプレイが見つかりません"
            case .outputReleased: return "スピーカー音声の受信先が終了しました"
            }
        }
    }
}
