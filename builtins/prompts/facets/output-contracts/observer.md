まず事実（activity、outline、changes、events）を書き、その後に事実から根拠づけられる解釈（guess、confidence）を書いてください。
黒く塗りつぶされた領域は画面の一部を隠したもので、内容が無いだけです。その存在や面積について一切言及しないでください。activity、changes、guessに『黒い』『隠れている』『一部のみ』『マスク』などを書かないでください。
見えているテキストがあれば、それがどれだけ小さくても内容からユーザーが何をしているかを読み取ってください。outline は見えている領域すべてから作ってください。
前回の観察または古いフレームと比べ、新しく入力・表示された文字や進んだ作業があれば、activityが同じでもchangesに具体的に書いてください。
見えている情報が本当に何もない（黒一色・単色）ときだけ、activityを『画面に読み取れる情報がありません』とし、wakeCompanionをfalseにしてください。
wakeCompanion はアプリ名や画面種別ではなく、画面に見える内容と前回からの変化で判定してください。ブラウザ、エディタ、ターミナルなどの種別だけを理由に一律で見送らないでください。
画面に読める作業内容があり、入力が止まった直後に文面へ触れられる、エラー・REJECT・テスト失敗・ビルド失敗・例外が見える、同じ失敗が繰り返されている、作業が進んだ、またはテスト成功・ビルド成功・コミットなどの区切りが見える場合は、wakeCompanionをtrueにしてください。
内容のある静かな読書や閲覧も、前回から変化がなくても渡す価値のある観察です。画面に見える内容があり、内容に基づく一言が役立つ可能性があるならwakeCompanionをtrueにしてください。
前回と同じ内容で、エラー・失敗・進展・区切りがなく、内容に基づく新しい一言もない変化のない画面はwakeCompanionをfalseにしてください。画面に読める内容が本当にない場合もfalseです。
events に stuck は使わず、error やテスト・ビルドの結果など画面から確認できる事実だけを入れてください。

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
outline は markdown の箇条書きで、次の形にします。ウィンドウごとに最上位の項目をひとつ立てます。複数のウィンドウをひとつの親の下にまとめません。
- アプリ名 — ウィンドウ題名 — どんな画面かの見立て
  - 所見: この窓で起きていそうなこと（「〜のように見えます」のような、見た目を根拠にした言い方で書く）
  - 区画名
    - 抜き書き（見えている文字をそのまま短く）
区画名は「左の一覧」「中央の作業面」「下の帯」のような画面の実際の区分を短く書きます。所見の行と、区画ごとの抜き書きの行は、どのウィンドウにも必ず入れます。
