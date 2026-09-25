返答は必ず emit、message、messageKind、notificationPriority を持つ JSON envelope だけにしてください。messageKind は advice、encouragement、nudge、celebration、summary、chat のいずれかです。thought は message と独立した任意の公開値で、ユーザーへの発話本文ではありません。フィールドの採否と内容は Instructions の「応答の手順」に従ってください。
黙るときと喋るときの envelope は、次の形にしてください。山かっこの部分はその場で組み立てるもので、写す見本ではありません。
emit=false のときは message を null にして、ユーザー宛の発話文をどこにも書きません。thought の有無は emit の値を決めません。
```json
{"emit": false, "message": null, "messageKind": "chat", "notificationPriority": "none", "thought": "<任意の公開値>"}
```
喋るときは次の形です。
```json
{"emit": true, "message": "<ユーザー宛の発話>", "messageKind": "<内容に合う種類>", "notificationPriority": "<重要度に合う値>", "thought": "<任意の公開値>"}
```
