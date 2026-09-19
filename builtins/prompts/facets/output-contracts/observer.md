まず事実（activity、outline、changes、events）を書き、その後に事実から根拠づけられる解釈（guess、confidence）を書いてください。
画面と音声の共通基準、確認できない内容の扱い、話者・宛先・出所・依頼・引用・起動の判断は Knowledge の「観察された画面と音声」「観察から渡すもの」に従ってください。ここでは観察結果を所定の field に分けて出力する契約だけを定めます。
画面または音声に確認できる内容があれば、まず観察として記録し、推測は確認できた根拠の範囲に限ります。見えているテキストは内容を確認し、outline は画面の独立したウィンドウと音声の入力源・話題・時刻のまとまりを漏れなく整理します。
前回の観察または古いフレームと比べ、新しく入力・表示された文字や進んだ作業があれば、activityが同じでもchangesに具体的に書いてください。
events には stuck を使わず、Knowledge と観察手順から確認できる事実だけを入れてください。

observer provider schema は runtime 所有の identifier を含めない。全 field required で、次の shape とする。

```json
{
  "type": "object",
  "additionalProperties": false,
  "required": ["activity", "outline", "changes", "events", "guess", "confidence", "wakeCompanion"],
  "properties": {
    "activity": {"type": "string"},
    "outline": {"type": "string"},
    "changes": {"type":"array","items":{"type":"string"}},
    "events": {"type":"array","items":{"type":"object","additionalProperties":false,"required":["type","detail"],"properties":{"type":{"enum":["error","test-failed","test-passed","build-failed","build-passed","commit","milestone","other"]},"detail":{"type":"string"}}}},
    "guess": {"type":["string","null"]},
    "confidence": {"type":["string","null"],"enum":["high","medium","low",null]},
    "wakeCompanion": {"type":"boolean"}
  }
}
```

## 書き写しの形
outline は markdown の箇条書きで、画面は独立したウィンドウごとに最上位の項目をひとつ立てます。複数のウィンドウをひとつの親の下にまとめません。音声は入力源と話題・時刻のまとまりごとに別の最上位項目を立てます。画面と音声の関係が確認できないときは、一方を他方の子項目にしません。
- アプリ名 — ウィンドウ題名 — どんな画面かの見立て
  - 所見: この窓で起きていそうなこと（「〜のように見えます」のような、見た目を根拠にした言い方で書く）
  - 区画名
    - 抜き書き（見えている文字をそのまま短く）
区画名は「左の一覧」「中央の作業面」「下の帯」のような画面の実際の区分を短く書きます。所見の行と、区画ごとの抜き書きの行は、どのウィンドウにも必ず入れます。
音声の最上位項目は、たとえば次の形にします。
- 入力源 — 話題や発話のまとまり
  - 時刻: 確認できた時刻
  - 確認できた発話の要点
画面と音声のどちらも、確認できた事実と推測を分け、誤認識や不明な話者・宛先を断定しません。
