# SilverHamuへの移行

製品名は **SilverHamu（シルバーハム）**。`Silver` は「銀の弾丸」に込めた性能への挑戦、`Hamu` はハムスター由来のブランド向けの造語。表示名は `SilverHamu`、CLI・パッケージ名は小文字の `silverhamu` とする。名称の由来は、実測で確認されていない性能の優位性を保証するものではない。

開発途中の仮称 `silveremu` も今回の変更で置き換える。製品ロゴには、回し車GIFに登場する銀のハムスター「ギンハムちゃん」の右向きの横顔を、同じ色鉛筆スタイルで使う。透過PNGの原本は [ginhamu_profile.png](../assets/branding/ginhamu_profile.png)。Ownerのヘッダー・アプリアイコン・スプラッシュにも反映する。

Ownerでは、提供された回し車GIFを処理中だけ画面中央に表示・再生する。Windowsのアニメーション設定が無効なら静止画と「処理中…」の文字を表示する。ファイル選択の入力待ちには重ねない。

## コマンドとSDK

| 対象 | 旧名 | 新名 |
|---|---|---|
| Rustパッケージ・CLI | `catdb` | `silverhamu` |
| TCPサービス | `catdb-server` | `silverhamu-server` |
| Windowsアプリ | `Catdb.Owner` | `SilverHamu.Owner` |
| SQLライブラリ・ホスト | `Catdb.Sql` / `Catdb.Sql.Host` | `SilverHamu.Sql` / `SilverHamu.Sql.Host` |
| Python配布名・import | `catdb-service-client` / `catdb_client` | `silverhamu-service-client` / `silverhamu_client` |
| Node.js・Ruby配布名 | `catdb-client` | `silverhamu-client` |
| Ruby require・名前空間 | `catdb` / `CatDB` | `silverhamu` / `SilverHamu` |
| Java artifact・パッケージ | `catdb-client` / `io.github.dhqboiler.catdb` | `silverhamu-client` / `io.github.dhqboiler.silverhamu` |
| .NETパッケージ・名前空間 | `Catdb.Client` / `Catdb` | `SilverHamu.Client` / `SilverHamu` |
| Goパッケージ | `catdb` | `silverhamu` |

スクリプトの実行名、SDKのimport・型名、プロジェクト参照を新名へ更新する。旧CLI名と旧SDK名の別名は提供しない。ビルド済みの旧実行ファイルが残る場合は、新名の出力を明示して実行する。

新しい配布名のレジストリ公開・予約は、このファイル変更には含まれない。未公開の版はチェックアウトからビルドする。GitHubリポジトリは `dhq-boiler/SilverHamu` へ改名済みで、資料内のGit URLも更新した。現在の作業ディレクトリは `C:\Git\SilverHamu`。Goモジュールは `clients/go/go.mod` の宣言に合わせて `github.com/dhq-boiler/catdb/clients/go` を引き続き使用する。リポジトリの改名だけではGoモジュールパスは変わらないため、インストール例とimportもこの宣言に合わせる。

Ownerは `apps/SilverHamu.Owner.sln` から開く。ソリューション内のプロジェクトと出力は `SilverHamu.*`。ネイティブDLLの場所を指定するMSBuildプロパティは `SilverHamuNativePath`。旧名のソリューション・プロジェクト・SDKソースは新名へ移行し、旧名のビルドキャッシュは削除した。

## 既存データとの互換性

次の識別子は既存データ・接続設定を維持するため変更しない。

- `.ndb`、`.catlink`、旧参照ファイル名 `catdb-current.txt` と既存バックアップの形式。
- バックアップ識別子 `CATDBBAK`、SQLカタログの `_catdb_owner/sql/v1/`、Raft内部キー。
- Rustライブラリ名 `newdbms`、ネイティブDLL名とC ABI。
- 設定・テスト・運用スクリプトの `CATDB_*` 環境変数。
- WindowsのアプリケーションID `dev.catdb.owner`。
- 過去の計測データと、比較用の `catdb` 計測キー、実験用のイメージ名・実行オプション。資料の本文・表では製品の表示名をSilverHamuに統一し、数値や原データは変更しない。

DBの変換や再作成は不要。プロトコルのバージョンも変更しない。
