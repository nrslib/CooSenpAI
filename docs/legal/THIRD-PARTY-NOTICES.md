# 第三者ライセンス表記

## habitua connectome-helper

同梱する `coosenpai-connectome` は habitua の `habitua` および `habitua-connectome` crate のソースからビルドしています。取り込み元は `third_party/habitua/SOURCE.txt` に記録しています。現在のスナップショットは commit `a3d83f295b87082a1fa0fecaa9c343e2a9f827b1` です。

- ライセンス: MIT OR Apache-2.0
- ライセンス全文: `licenses/habitua/LICENSE-MIT`、`licenses/habitua/LICENSE-APACHE`
- 変更: CooSenpAI のビルドに必要なソースを取り込み、アプリの判断役 helper としてビルド

## MaleCNS v1.0

同梱する学習済みファイル `connectome/malecns-frozen-response-learning-artifact-r14.json` は、Janelia が公開した MaleCNS v1.0 由来の配線データを rate pack に変換し、応答モデルを学習して作成したものです。rate pack 自体はアプリに同梱していません。開発者設定で判断役を有効にし、pack が無い場合は、初回に利用者の操作で habitua v0.1.0 Release から約 605 MB の pack をダウンロードできます。必要な pack manifest の SHA-256 は `29ec59f629013be3137e4a084446db70389ffc42f9355ee179c724334b2da082` です。

- 提供者: Janelia Research Campus
- 出典: [MaleCNS](https://male-cns.janelia.org/)
- 配布元: [habitua v0.1.0 Release](https://github.com/nrslib/habitua/releases/tag/v0.1.0)。同 Release の `malecns-v1.0-rate-full.distribution.json` に元データ、変換方法、ライセンスが記録されています。
- 論文: Berg et al., *Cell* (2026), DOI: [10.1016/j.cell.2026.08.015](https://doi.org/10.1016/j.cell.2026.08.015)
- ライセンス: [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)
- ライセンス全文: [CC-BY-4.0-legalcode.txt](CC-BY-4.0-legalcode.txt)
- 変更: MaleCNS v1.0 のデータを発火率シミュレーション用 rate pack に変換し、CooSenpAI の観測に応答する学習済みファイルを作成

## WeSpeaker voxceleb ResNet34-LM Core ML モデル

`models/speaker-id/WespeakerResNet34LM.mlpackage` は WeSpeaker ResNet34-LM の重みを Core ML ML Program へ変換したものです。重みは Creative Commons Attribution 4.0 International（CC BY 4.0）で提供されます。変換後のモデルであることを明示します。

- 原著者・プロジェクト: WeSpeaker project / WeNet authors
- モデル: WeSpeaker voxceleb ResNet34-LM (`wespeaker-voxceleb-resnet34-LM`)
- 学習データ: VoxCeleb2 Dev（出典のモデルカードによる）
- 出典: [WeSpeaker モデルカード](https://huggingface.co/Wespeaker/wespeaker-voxceleb-resnet34-LM/tree/f0c48c298fd835726c27956a5d617bad7115627e)、revision `f0c48c298fd835726c27956a5d617bad7115627e`
- 論文著者: Hongji Wang、Chengdong Liang、Shuai Wang、Zhengyang Chen、Binbin Zhang、Xu Xiang、Yanlei Deng、Yanmin Qian
- ライセンス: [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/)
- ライセンス全文: [CC-BY-4.0-legalcode.txt](CC-BY-4.0-legalcode.txt)
- 変更: 公開 PyTorch 重みを Core ML ML Program へ変換。macOS 13、float32、CPU-only、非量子化、入力形状 `[1, 200, 80]` で固定

変換後のモデルにも CC BY 4.0 を適用します。アプリ本体のライセンスや利用規約は、このモデルについて CC BY 4.0 が認める権利を制限しません。

WeSpeaker の学習・実装コードは Apache License 2.0 です。このライセンス表記はモデル重みのライセンスとは別です。

- プロジェクト: [WeSpeaker](https://github.com/wenet-e2e/wespeaker)
- 使用したコードの commit: `dfa741957e5c11f477623b6e583d67d0af25ee88`
- ライセンス: Apache License 2.0
- ライセンス全文: [WeSpeaker-Apache-2.0.txt](WeSpeaker-Apache-2.0.txt)
