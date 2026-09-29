# 第三者ライセンス表記

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
