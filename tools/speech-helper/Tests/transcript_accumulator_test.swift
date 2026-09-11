import CoreMedia
import Foundation

func speechTranscription(_ text: String, start: Int64 = 0, end: Int64 = 48_000, isFinal: Bool = false) -> SpeechTranscription {
    SpeechTranscription(
        text: text,
        audioRange: CMTimeRange(start: CMTime(value: start, timescale: 48_000), end: CMTime(value: end, timescale: 48_000)),
        resultsFinalizationTime: CMTime(value: isFinal ? end : 0, timescale: 48_000)
    )
}

func testTranscriptAccumulator() {
    runTest("volatile の先頭変更・短縮・句読点挿入は最新全文で置換する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        for text in ["これは前半です", "これは前半でした", "こんにちは世界", "こんにちは、世界", "こんにちは", "別の先頭"] {
            try transcript.record(speechTranscription(text))
            expect(transcript.text == text && transcript.finalizedText.isEmpty, "仮説を連結・確定しない")
        }
    }

    runTest("確定範囲を保持し、後続の volatile だけを置換する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("最初の文をここで話します。", isFinal: true))
        for text in ["そして", "そして都の", "そして続きの文をここで話しますま", "そして続きの文をここで話します。"] {
            try transcript.record(speechTranscription(text, start: 216_000, end: 360_000))
            expect(transcript.text == "最初の文をここで話します。" + text, "確定済みの前半を消さない")
        }
        try transcript.record(speechTranscription("そして続きの文をここで話します。", start: 216_000, end: 360_000, isFinal: true))
        expect(transcript.finalizedText == "最初の文をここで話します。そして続きの文をここで話します。", "2区間の確定本文を保持する")
        expect(!transcript.hasUnfinalizedText, "確定結果で volatile を取り除く")
    }

    runTest("離れた volatile 区間を保持し、音声範囲の変化だけでは確定しない") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("古い仮説"))
        try transcript.record(speechTranscription("新しい仮説", start: 96_000, end: 144_000))
        expect(transcript.text == "古い仮説新しい仮説" && transcript.finalizedText.isEmpty, "別区間を単一の partial で上書きしない")
        try transcript.record(speechTranscription("前半の訂正", start: 0, end: 24_000))
        expect(transcript.text == "前半の訂正新しい仮説", "短縮した訂正でも別区間は保持する")
        try transcript.record(speechTranscription("", start: 0, end: 24_000))
        expect(transcript.text == "新しい仮説", "空の訂正は重なる区間だけを消す")
        try transcript.finalize(through: CMTime(value: 144_000, timescale: 48_000))
        expect(transcript.finalizedText == "新しい仮説" && !transcript.hasUnfinalizedText, "本文の再通知なしに確定する")
    }

    runTest("後続の volatile 結果の確定時刻で前の区間を昇格する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("前半"))
        let later = SpeechTranscription(
            text: "後半", audioRange: CMTimeRange(start: CMTime(value: 1, timescale: 1), duration: CMTime(value: 1, timescale: 1)),
            resultsFinalizationTime: CMTime(value: 1, timescale: 1)
        )
        expect(!later.isFinal, "後続の結果自身はまだ volatile")
        try transcript.record(later)
        expect(transcript.finalizedText == "前半" && transcript.text == "前半後半", "確定時刻より前の結果を再通知なしに昇格する")
        try transcript.finalize(through: CMTime(value: 2, timescale: 1))
        expect(transcript.finalizedText == "前半後半" && !transcript.hasUnfinalizedText, "後半も時刻で確定する")
    }

    runTest("volatile の結合と開始位置変更は重なる区間だけを置換する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("末尾", start: 144_000, end: 192_000))
        try transcript.record(speechTranscription("先頭"))
        try transcript.record(speechTranscription("中間", start: 48_000, end: 96_000))
        expect(transcript.text == "先頭中間末尾", "受信順ではなく音声時刻順に表示する")
        try transcript.record(speechTranscription("統合した訂正", start: 1, end: 95_999))
        expect(transcript.text == "統合した訂正末尾", "重なる2区間を新しい結果で置換する")
        try transcript.finalize(through: CMTime(value: 48_000, timescale: 48_000))
        expect(transcript.finalizedText.isEmpty, "時刻が区間の途中なら文字を推測して分割しない")
        try transcript.finalize(through: CMTime(value: 192_000, timescale: 48_000))
        expect(transcript.finalizedText == "統合した訂正末尾", "保持した全区間を順に確定する")
    }

    runTest("空の volatile は未確定本文だけを消す") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("確定済み", isFinal: true))
        try transcript.record(speechTranscription("取り消される仮説", start: 48_000, end: 96_000))
        try transcript.record(speechTranscription("", start: 48_000, end: 96_000))
        expect(transcript.text == "確定済み" && !transcript.hasUnfinalizedText, "空結果を古い仮説で補わない")
    }

    runTest("短い確定結果への修正を採用する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "en-US"))
        try transcript.record(speechTranscription("Hello, world again!", end: 144_000))
        try transcript.record(speechTranscription("hello world.", end: 96_000, isFinal: true))
        expect(transcript.text == "hello world." && !transcript.hasUnfinalizedText, "最長文字列や削除済みの末尾を復活させない")
    }

    runTest("確定済み範囲の再通知で追記・変更・後半消去をしない") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "en-US"))
        try transcript.record(speechTranscription("hello", isFinal: true))
        try transcript.record(speechTranscription("world", start: 96_000, end: 144_000))
        for isFinal in [false, true] {
            try transcript.record(speechTranscription("重複通知", isFinal: isFinal))
            expect(transcript.text == "hello world", "immutable な確定範囲を維持する")
        }
    }

    runTest("異なる timescale の隣接範囲を整数時刻で連結する") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(SpeechTranscription(text: "前半", audioRange: CMTimeRange(
            start: CMTime(value: 897, timescale: 100), duration: CMTime(value: 63, timescale: 100)
        ), resultsFinalizationTime: CMTime(value: 960, timescale: 100)))
        try transcript.record(speechTranscription("後半", start: 460_800, end: 541_440, isFinal: true))
        expect(transcript.text == "前半後半", "Double の丸め誤差を境界判定へ持ち込まない")
    }

    runTest("確定範囲へ1サンプルでも食い込む新結果は失敗にする") {
        for isFinal in [false, true] {
            var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
            try transcript.record(speechTranscription("確定済み", isFinal: true))
            do {
                try transcript.record(speechTranscription("重なる新結果", start: 47_999, end: 96_000, isFinal: isFinal))
                expect(false, "確定範囲を勝手に再解釈しない")
            } catch SpeechTranscriptError.overlapsFinalizedAudio {}
            expect(transcript.text == "確定済み", "拒否した結果で本文を変更しない")
        }
    }

    runTest("不正な音声時刻を本文へ採用しない") {
        for range in [CMTimeRange.invalid, CMTimeRange.zero,
                      CMTimeRange(start: .negativeInfinity, duration: .positiveInfinity),
                      CMTimeRange(start: CMTime(value: -1, timescale: 48_000), duration: CMTime(value: 1, timescale: 1))] {
            var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
            do {
                try transcript.record(SpeechTranscription(text: "不正", audioRange: range, resultsFinalizationTime: .zero))
                expect(false, "無効・非数値・負の開始・ゼロ長の範囲を拒否する")
            } catch SpeechTranscriptError.invalidAudioRange {}
        }
    }

    runTest("不正な確定時刻では本文を変更しない") {
        for time in [CMTime.invalid, .indefinite, .positiveInfinity, CMTime(value: -1, timescale: 1)] {
            var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
            try transcript.record(speechTranscription("本文"))
            do {
                try transcript.record(SpeechTranscription(
                    text: "不正な訂正", audioRange: speechTranscription("").audioRange,
                    resultsFinalizationTime: time
                ))
                expect(false, "不正な確定時刻を拒否する")
            } catch SpeechTranscriptError.invalidFinalizationTime {}
            expect(transcript.text == "本文" && transcript.finalizedText.isEmpty, "検証失敗では状態を変更しない")
        }
    }

    runTest("同じ語の別区間は保持し、言語に応じた単語境界を使う") {
        for (locale, expected) in [("ja-JP", "はいはい"), ("en-US", "はい はい")] {
            var transcript = SpeechTranscript(locale: Locale(identifier: locale))
            try transcript.record(speechTranscription("はい", isFinal: true))
            try transcript.record(speechTranscription("はい", start: 48_000, end: 96_000, isFinal: true))
            expect(transcript.text == expected, "同じ文字列という理由で別発話を削除しない")
        }
        var transcript = SpeechTranscript(locale: Locale(identifier: "en-US"))
        try transcript.record(speechTranscription(" hello ", isFinal: true))
        try transcript.record(speechTranscription(" world\n", start: 48_000, end: 96_000, isFinal: true))
        expect(transcript.text == " hello  world\n", "元の空白を保持する")
    }

    runTest("空の確定結果は元の volatile を残さない") {
        var transcript = SpeechTranscript(locale: Locale(identifier: "ja-JP"))
        try transcript.record(speechTranscription("消される仮説"))
        try transcript.record(speechTranscription("", isFinal: true))
        expect(transcript.finalizedText.isEmpty && transcript.text.isEmpty && !transcript.hasUnfinalizedText, "無音や訂正を成功本文で補わない")
    }
}
