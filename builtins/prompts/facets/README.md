# プロンプトのファセット構成（作業前に必ず読む）

このディレクトリは faceted prompting の流儀で system prompt を組み立てる材料です。ファセットごとに責務が分かれています。companion では persona だけをユーザーが切り替え、残りの層は全 companion persona に共通です。observer は別の役割なので、knowledge・instructions・output-contracts・policies のうち observer に必要な内容だけを選びます。正式な契約は `docs/contracts/prompts.md` にあり、この README はその要約です。食い違いがあれば契約が正です。

## ファセットの責務

| ディレクトリ | 責務 | 位置づけ |
|---|---|---|
| `personas/` | 話し方と振る舞いだけ。口調、反応の順序と温度。判断規則や製品知識は書かない | ユーザーが切り替える層。設定で選び、`~/.coosenpai/personas/<id>.md` に custom persona を置ける |
| `knowledge/` | 製品コンセプトと判断の前提。`observation.md` は両役割の共通前提、`observation_companion.md` は companion 専用の判断基準 | companion は全ファイル、observer は `observation.md` だけを読む。アプリ同梱でユーザー領域からは読まない |
| `instructions/` | 手順と役割宣言。`observer.md` は観察エージェント専用。`observer-audio.md` と `microphone-commands.md` は該当する observer 呼び出しに限る | companion は `observer.md`、`observer-audio.md`、`microphone-commands.md` を除外。observer は `observer.md` と必要な条件付き指示を使う |
| `output-contracts/` | 出力フィールドの意味と書式の契約（雛形を含む）。companion と observer の役割別 | companion と observer で指定ファイルを分ける |
| `policies/` | 振る舞いの戒め。宣言的な規範を条文として書く。ファイル名の昇順で連結される | companion は全ファイル、observer は `assertiveness.md` と `speech.md` を除外 |

companion persona を切り替えても、companion の共通層は変わりません。どの companion persona でも同じであるべきもの（製品の意味づけ、判断の手順、出力の契約、振る舞いの規範）は persona に書かず、共通層のどれかに置きます。persona に書いた判断規則は、ペルソナを替えた途端に消えます。逆に、口調や反応の温度のようにペルソナごとに違ってよいものだけを persona に書きます。observer には persona と companion 専用の知識・規範を渡しません。

## 合成順

companion（Coo）の system prompt は次の順で連結されます。

1. 選択した persona の本文
2. `## Knowledge` と `knowledge/` の名前順連結
3. 動的文脈（表示名、積極性、記憶など。runtime が生成）
4. `## Instructions` と `instructions/` の名前順連結（`observer.md`、`observer-audio.md`、`microphone-commands.md` を除く）
5. `## Output` と `output-contracts/companion.md` の本文
6. `## Policy` と `policies/` の名前順連結。必ず末尾

observer の system prompt は、`## Knowledge` と共通前提の `knowledge/observation.md`、動的文脈、`## Instructions` と `instructions/observer.md`、`## Output` と `output-contracts/observer.md`、observer 用 `## Policy` の順です。音声がある場合は `observer-audio.md`、許可されたマイク候補を分類する場合は `microphone-commands.md` を `## Instructions` 内に追加します。observer の Policy は名前順の連結から `assertiveness.md` と `speech.md` を除きます。companion 専用の knowledge と persona は observer に渡しません。画面と音声の共通基準は `knowledge/observation.md` に一か所だけ置き、companion 専用の判断基準は `knowledge/observation_companion.md` に置きます。

選択された各ディレクトリの Markdown は名前順に連結され、合成側から区切りは挿入されません。ファイルの除外規則は役割ごとに固定されています。ファイルを足すときは名前で順序が決まることと、末尾の改行が連結後の空行になることに注意してください。

## 変更のしかた

- 条文を変えたら `prompts-eval/`（promptfoo）で変更前後を比べます。3 プロバイダ、repeat 3 が目安です。
- 合成後の全文は golden fixture（`fixtures/prompts/`）で byte 一致させています。ファセットを変えたら fixture を同期し、`cargo test --workspace` を通してください。
- 共通層の変更はエバルで効果を確かめてから採用します。
- ペルソナの書き方の規範は `prompts-eval/CLAUDE.md` にあります。決め台詞ではなくスタンスを書き、根拠のない性質を足しません。

## ここに置かないもの

- この README 以外のファイルをこのディレクトリ直下に置かないでください。各サブディレクトリの Markdown はビルド時に合成対象として選択されます。
- 役割呼称（相棒、先輩、見守り）は使いません。名前は Coo、機能名は Vision AI / Hearing AI です。
