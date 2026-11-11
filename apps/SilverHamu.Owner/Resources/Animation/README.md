# 回し車アニメーションの生成元

ユーザーが理想に近いと指定した、一番最初の12コマGIFを `original.gif` に保存している。

走りと車輪の絵が9コマ目から最初の並びに戻ることを踏まえ、最初の8コマで循環させる。開始位置は元の7コマ目で、順番は7・8・1・2・3・4・5・6。プレビューは4段階を同じ列に置き、各2コマとする。

元の画像の不透明ピクセルのRGB値、透過、各110msの表示時間をそのまま保持する。1周0.88秒・8コマ・約9.1fps。部品化して動かす方式や、元絵の輪郭を崩した動画補間案は採用していない。元絵の大きく走る動きを優先し、元の12コマの途中から先頭へ戻るループ位置を直した版。

PillowをインストールしたPythonで次を実行する。

```powershell
& apps/SilverHamu.Owner/Resources/Animation/render.ps1
```

`artifacts/owner-animation-review/original-restored-loop.gif` と `original-restored-preview.png` を出力する。採用するGIFは `Resources/Images/silverhamu_loading.gif` へコピーする。通常のOwnerのビルドにPythonや画像生成は不要。

再構成時に、8コマ・110ms・無限ループ・透過と、すべての元ピクセルが一致することを検証する。
