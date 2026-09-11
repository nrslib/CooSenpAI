import Foundation

private var testCount = 0

func expect(_ condition: @autoclosure () -> Bool, _ message: String, file: StaticString = #filePath, line: UInt = #line) {
    if !condition() {
        FileHandle.standardError.write(Data("FAIL \(file):\(line): \(message)\n".utf8))
        exit(1)
    }
}

func runTest(_ name: String, _ test: () throws -> Void) {
    do { try test() }
    catch { expect(false, "\(name): \(error)") }
    testCount += 1
    print("PASS \(name)")
}

@main
struct SpeechHelperTests {
    static func main() {
        if CommandLine.arguments.count == 4, CommandLine.arguments[1] == "--real-early-finish" {
            testRealEarlyFinish(engine: try! SpeechEngine.resolve(CommandLine.arguments[2]), mode: CommandLine.arguments[3])
        }
        if CommandLine.arguments.contains("--expect-failure") {
            expect(false, "最適化ビルドでも失敗を検出する")
        }
        testTranscriptAccumulator()
        testSpeechEngine()
        testRecognitionSession()
        testAudioConverter()
        testWavInput()
        testWavDump()
        print("Speech helper: \(testCount) tests passed")
    }
}
