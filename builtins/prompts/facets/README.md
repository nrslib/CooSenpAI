# プロンプトのファセット構成（作業前に必ず読む）

このディレクトリは faceted prompting の流儀で system prompt を組み立てる材料です。ファセットごとに責務が分かれています。いちばん大事な区別は、persona はユーザーが切り替えて設定する層で、それ以外の 4 つは全ペルソナに共通する層だということです。共通層は開発側で普通に編集します。正式な契約は `docs/contracts/prompts.md` にあり、この README はその要約です。食い違いがあれば契約が正です。

## ファセットの責務

| ディレクトリ | 責務 | 位置づけ |
|---|---|---|
| `personas/` | 話し方と振る舞いだけ。口調、反応の順序と温度。判断規則や製品知識は書かない | ユーザーが切り替える層。設定で選び、`~/.coosenpai/personas/<id>.md` に custom persona を置ける |
| `knowledge/` | 製品コンセプトと判断の前提。CooSenpAI の名前の意味（See / Encourage / Nudge / Pleasure）、音声の出どころの意味づけなど | 全ペルソナ共通の層。アプリに同梱し、ユーザー領域からは読まない。開発側で編集する |
| `instructions/` | 手順と役割宣言。観察を読む、話すかを決める、種類を選ぶ、本文を組む、最後に文体を点検する、という手続きだけ。`observer.md` は観察エージェント専用 | 同上 |
| `output-contracts/` | 出力フィールドの意味と書式の契約（雛形を含む）。いまは observer 用のみ | 同上 |
| `policies/` | 振る舞いの戒め。宣言的な規範を条文として書く。ファイル名の昇順で連結される | 同上 |

切り替わるのは persona だけです。ユーザーがペルソナを替えても、knowledge、instructions、output-contracts、policies はそのまま残ります。だから、どのペルソナでも同じであるべきもの（製品の意味づけ、判断の手順、出力の契約、振る舞いの規範）は persona に書かず、共通層のどれかに置きます。persona に書いた判断規則は、ペルソナを替えた途端に消えます。逆に、口調や反応の温度のようにペルソナごとに違ってよいものだけを persona に書きます。

## 合成順

companion（Coo）の system prompt は次の順で連結されます。

1. 選択した persona の本文
2. `## Knowledge` と `knowledge/` の名前順連結
3. 動的文脈（表示名、積極性、記憶など。runtime が生成）
4. `## Instructions` と `instructions/` の名前順連結（`observer.md` を除く）
5. `## Policy` と `policies/` の名前順連結。必ず末尾

observer の system prompt は、動的文脈、`instructions/observer.md`、`output-contracts/`、`policies/` の順です。persona と knowledge は observer に渡しません。

各ディレクトリの Markdown は名前順に連結されるだけで、合成側から区切りは挿入されません。ファイルを足すときは名前で順序が決まることと、末尾の改行が連結後の空行になることに注意してください。

## 変更のしかた

- 条文を変えたら `prompts-eval/`（promptfoo）で変更前後を比べます。3 プロバイダ、repeat 3 が目安です。
- 合成後の全文は golden fixture（`fixtures/prompts/`）で byte 一致させています。ファセットを変えたら fixture を同期し、`cargo test --workspace` を通してください。
- 共通層の変更はエバルで効果を確かめてから採用します。
- ペルソナの書き方の規範は `prompts-eval/CLAUDE.md` にあります。決め台詞ではなくスタンスを書き、根拠のない性質を足しません。

## ここに置かないもの

- この README 以外のファイルをこのディレクトリ直下に置かないでください。各サブディレクトリの Markdown はビルド時に全部連結されます。
- 役割呼称（相棒、先輩、見守り）は使いません。名前は Coo、機能名は Vision AI / Hearing AI です。
