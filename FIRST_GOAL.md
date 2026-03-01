動画と音声の Fragmented MP4 を一つの動画+音声の Fragmented MP4 にするプログラムを Rust で書いてください。

## 要件

* ストリーム情報は以下のJSONファイルで受け取る。
  * `{"streams": [{"format": "mp4", "codecs": ["hev1.2.4.L123.B0"], "init": "stream1.init.m4s", "chunks": ["stream1.chunk01.m4s", "stream1.chunk02.m4s"]}]}`
* できあがったファイルには sidx チャンクを含めること。
* 成果物において、init+sidx部分と実ストリーム部分は別々のファイル、out.init.m4s と out.data.m4s として出力すること
  * 配信時に out.init.m4s と out.data.m4s は結合されて配信されるので、それを想定したファイルを出力すること
  * (つまり、ファイルが正しくできたかテストする場合には事前に `cat out.init.m4s out.data.m4s > out.m4s` などとして結合してから再生すること)
* 成果物(を結合したもの)は Fragmented MP4 (sidxチャンク) に対応した一般的なプレーヤーで再生できるようになっていること。
* 元々のinit/chunksファイルデータを(ヘッダなど含めて)すべて**無変更のまま**成果物に含め、どのファイルが成果物のどの部分に入っているかを out.sources.json として出力すること。
  * 例: `{"files": [{"source": "chunk.0.m4s", "dest": {"type": "data", "offset": 0, "length": 12345}}, {"source": "chunk.1.m4s", "dest": {"type": "data", "offset": 12345, "length": 67890}}]}`
  * ただ含めればいいというわけでもない。各ファイルのデータをヘッダーなどでうまくごまかしてそのまま使うこと。
* mp4の読み書き部分にあたって、外部ライブラリは使わないこと。