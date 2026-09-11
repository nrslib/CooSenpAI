import Foundation

func testSpeechEngine() {
    runTest("OS に応じた自動選択と SFSpeechRecognizer の強制指定") {
        let automatic = try SpeechEngine.resolve("auto")
        let sf = try SpeechEngine.resolve("sf")
        expect(sf == .sf && sf.name == "SFSpeechRecognizer", "26 以降でも sf を選べる")
        if #available(macOS 26.0, *) {
            let analyzer = try SpeechEngine.resolve("analyzer")
            expect(automatic == .analyzer && analyzer.name == "SpeechAnalyzer", "26 以降は analyzer")
        } else {
            expect(automatic == .sf, "26 未満は sf")
            do {
                _ = try SpeechEngine.resolve("analyzer")
                expect(false, "26 未満で analyzer を生成しない")
            } catch let failure as SpeechAnalysisFailure {
                expect(failure.kind == "arguments", "未対応 OS の指定を拒否する")
            }
        }
    }
    runTest("未知の engine を自動選択で補わない") {
        do {
            _ = try SpeechEngine.resolve("invalid")
            expect(false, "未知の指定を拒否する")
        } catch let failure as SpeechAnalysisFailure {
            expect(failure.kind == "arguments", "引数エラーを返す")
        }
    }
}
