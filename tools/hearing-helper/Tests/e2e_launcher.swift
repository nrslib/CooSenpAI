import CoreGraphics
import Foundation

private func report(_ message: String) {
    FileHandle.standardError.write(Data("\(message)\n".utf8))
}

private func processColumn(_ column: String, pid: pid_t) -> String? {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/bin/ps")
    process.arguments = ["-p", String(pid), "-o", "\(column)="]
    let output = Pipe()
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    do {
        try process.run()
        process.waitUntilExit()
    } catch {
        return nil
    }
    guard process.terminationStatus == 0 else { return nil }
    return String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)?
        .trimmingCharacters(in: .whitespacesAndNewlines)
}

let arguments = CommandLine.arguments
let runIDText = arguments.count == 2 ? arguments[1] : ""
guard arguments.count == 2,
      UUID(uuidString: runIDText) != nil else {
    report("hearing E2E の実行識別子が不正です")
    exit(1)
}

let e2eRootURL = Bundle.main.bundleURL
    .appendingPathComponent("../hearing-e2e")
    .standardizedFileURL
let runDirectoryURL = e2eRootURL
    .appendingPathComponent("runs", isDirectory: true)
    .appendingPathComponent(runIDText, isDirectory: true)
let e2eConfigURL = runDirectoryURL.appendingPathComponent("config")

private func configurationValue(_ key: String) -> String? {
    guard let config = try? String(contentsOf: e2eConfigURL, encoding: .utf8) else { return nil }
    return config.split(whereSeparator: \.isNewline)
        .first(where: { $0.hasPrefix("\(key)=") })
        .map { String($0.dropFirst(key.count + 1)) }
}

guard configurationValue("run_id") == runIDText else {
    report("hearing E2E の実行識別子が設定と一致しません")
    exit(1)
}

private func requestScreenCaptureAccessFromTestApp() {
    guard configurationValue("mode") == "screen-auth" else {
        return
    }
    guard !CGPreflightScreenCaptureAccess() else { return }
    _ = CGRequestScreenCaptureAccess()
}

let appPIDPath = runDirectoryURL.appendingPathComponent("app.pid")
let appPID = ProcessInfo.processInfo.processIdentifier
guard let appStartedAt = processColumn("lstart", pid: appPID),
      let appExecutable = processColumn("comm", pid: appPID),
      !appStartedAt.isEmpty,
      !appExecutable.isEmpty else {
    report("hearing E2E の app PID の同一性情報を取得できませんでした")
    exit(1)
}
do {
    try Data("\(appPID)|\(runIDText)|\(appStartedAt)|\(appExecutable)\n".utf8)
        .write(to: appPIDPath, options: .atomic)
} catch {
    report("hearing E2E の app PID を記録できませんでした")
    exit(1)
}

if #available(macOS 10.15, *) {
    requestScreenCaptureAccessFromTestApp()
}

// 音声認識・画面収録の認可主体として、テストアプリを runner の終了まで保持する。
let runner = Process()
runner.executableURL = URL(fileURLWithPath: "/bin/sh")
runner.arguments = [
    Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/runner.sh").path,
    runIDText,
]
do {
    try runner.run()
    runner.waitUntilExit()
    exit(runner.terminationStatus)
} catch {
    report("hearing E2E runner を起動できませんでした")
    exit(1)
}
