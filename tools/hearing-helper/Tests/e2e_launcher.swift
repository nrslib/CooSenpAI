import Foundation

// 音声認識・画面収録の認可主体として、テストアプリを終了まで保持する。
let runner = Process()
runner.executableURL = URL(fileURLWithPath: "/bin/sh")
runner.arguments = [Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/runner.sh").path]
do {
    try runner.run()
    runner.waitUntilExit()
    exit(runner.terminationStatus)
} catch {
    FileHandle.standardError.write(Data("hearing E2E runner を起動できませんでした\n".utf8))
    exit(1)
}
