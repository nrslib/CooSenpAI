import Foundation

// LaunchServices から起動し、音声認識の認可主体を用途説明のあるテストアプリにする。
let arguments = Array(CommandLine.arguments.dropFirst())
guard arguments.count >= 2 else { exit(2) }
let process = Process()
process.executableURL = URL(fileURLWithPath: arguments[0])
process.arguments = Array(arguments.dropFirst())
do {
    try process.run()
    process.waitUntilExit()
    exit(process.terminationStatus)
} catch {
    FileHandle.standardError.write(Data("WAV テストを起動できませんでした\n".utf8))
    exit(1)
}
