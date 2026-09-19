# 音声入力の固定 WAV

## Issue #125 の fixture

`two-utterances-issue-125.wav` は、Issue #125 の「テステスマイクテス」を二度発話し、発話の間に3秒の無音を入れた TTS fixture である。

- 48,000 Hz / mono / Float32 PCM、306,164 frames（6.378417秒）
- 二つの発話はそれぞれ81,082 frames、間の無音は144,000 frames（3.000秒）
- SHA-256: `4255ab1ee8cfda15fa8106067b04cfc28846812bd84ac10f25b0b618e3ed5d16`
- 音声の生成文: `テステスマイクテス`、`テステスマイクテス`

生成手順は `tools/speech-helper/prepare-issue-125-wav.sh` に記録している。macOS の `say -v Kyoko -r 100` で各発話を AIFF に生成し、音量を8倍にして48 kHz / monoの Float32 PCMへ変換する。`prepare-issue-125-pcm.py` で各発話末尾のゼロフレームを除去し、二つの発話の間へ144,000個の無音フレームを挿入する。再生成する場合は次を実行する。

```sh
tools/speech-helper/prepare-issue-125-wav.sh tools/speech-helper/Tests/Fixtures/two-utterances-issue-125.wav
```

認識結果は engine ごとに安定した A・B を定義し、句読点と空白を除いて `A+B` の全文一致を検証する。

- SpeechAnalyzer: A=`テステスマイクテス`、B=`テステスマイクテス`
- SFSpeechRecognizer: A=`テステスマイクテス`、B=`テステスマイクテス`

同じ engine の誤った一回分だけの final や、先行本文を含まない後半だけの final は、この A+B の一致検証を通過しない。partial の先頭一致は volatile な結果に依存するため検証条件にしない。

## 実マイクの補助 fixture

`two-utterances-microphone.wav` は利用者が提供した実マイク録音から作成した補助 fixture である。

- 48,000 Hz / mono / Float32 PCM、873,600 frames（18.2秒）
- SHA-256: `d4e0f6abb11e1518e95ed9aa275e2b0e13429ef65a7656f4af578d22520d282b`
- `prepare-mic-e2e-wav.py` で、提供された `segment-microphone-2.wav` 全体、区間1の実環境音3.5秒、`segment-microphone-3.wav` 全体、区間1の実環境音1.5秒を連結した

実マイク fixture は元の環境音を含むため、Issue #125 の既定 E2E 入力には使わない。`tools/speech-helper/test.sh` の既定入力は Issue #125 fixture であり、macOS 26 以降では SpeechAnalyzer と SFSpeechRecognizer、26 未満では SFSpeechRecognizer を実行する。両 engine とも、ready・final・closed の一回性、finish から final まで30秒以内、全 PCM フレーム転送、A+B の全文一致を5回連続で検証する。
