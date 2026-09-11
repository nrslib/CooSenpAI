# 実マイクの音声入力回帰テスト

`two-utterances-microphone.wav` は利用者が提供した実マイク録音から作成した、SpeechAnalyzer と SFSpeechRecognizer の実認識テスト用 fixture。

- 48,000 Hz / mono / Float32 PCM、873,600 frames（18.2 秒）
- SHA-256: `d4e0f6abb11e1518e95ed9aa275e2b0e13429ef65a7656f4af578d22520d282b`
- 期待本文: `最初の文をここで話しますそして続きの文をここで話します`
- 比較時に除くのは句読点と空白のみ。文字の追加・欠落・同音異字は不一致。

`prepare-mic-e2e-wav.py` で、提供された `segment-microphone-2.wav` 全体、区間1の実環境音3.5秒、`segment-microphone-3.wav` 全体、区間1の実環境音1.5秒を連結した。区間2・3の前後にもともとある環境音も保持している。元の連続録音の時系列復元ではない。

音声の切り詰め、音量変更、リサンプル、合成音声・デジタル無音への置換は行っていない。元の区間3は WAV ヘッダーが未確定だったため、`ffmpeg` が読み出せる PCM 全体を使用した。元ファイルのハッシュと連結位置は `microphone-replay.json` に記録している。

`tools/speech-helper/test.sh` はこの WAV を実時間で投入し、stdin の `finish` を経由して SpeechAnalyzer の全文一致と、両 engine の final 一回性・30秒期限・全 PCM フレームの転送を5回連続で検証する。macOS 13.0 以降、macOS 26 API を含む Swift SDK、日本語モデル、Python 3、`ffprobe` が必要。26 以降では analyzer と sf をそれぞれ5回、26 未満では sf だけを実行する。SpeechAnalyzer は対応ハードウェアが必要。モデル未導入の場合、helper が Apple の API でモデルを準備するためネットワークが必要になる場合がある。テストは用途説明を持つ SpeechE2E.app を LaunchServices から起動する。WAV テストはマイク権限を要求しない。sf は音声認識権限が必要で、未決定なら要求する。sf は前半消失と認識精度の違いを許容し、final に後半の「続きの文」が含まれること、全 PCM 転送、final 一回性と30秒期限を検証する。sf の全文一致は保証しない。結果の prefix_lost に消失の有無を記録する。
