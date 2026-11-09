# catdb owner

SSMSのように、左側でテーブルと列を参照し、右側でSQLを書いて結果を表示する.NET MAUI Windowsデスクトップアプリ。

## 起動と操作

Visual Studio 2026 Communityで `../Catdb.Owner.sln` を開き、`Catdb.Owner` をスタートアッププロジェクトにしてWindows Machineで起動する。.NET 10とMAUI Windowsワークロードが必要。

初回ビルド前、またはRust側を変更したときはリポジトリルートでネイティブDLLをビルドする。Rustツールチェーンが必要で、この環境では既存の `.tools` を使用する。

```powershell
./scripts/build-owner-native.ps1
```

生成先は `target/owner-native/release/newdbms.dll`。アプリとテストのビルド時に各出力フォルダーへコピーされる。`CatdbNativePath` で互換DLLの場所を明示することもできる。古い `target/release/newdbms.dll` へ暗黙にフォールバックしない。組み込み接続はWindows x64版。

ビルド済みの開発用アプリは `bin/Debug/net10.0-windows10.0.19041.0/win-x64/Catdb.Owner.exe`。出力フォルダーの依存ファイルと一緒に利用する。自己完結パッケージ・インストーラーは未作成。

TCP サービスに SQL ホストが設定されている場合は、Owner がサーバーへ SQL 文を送る。KV 専用サービスでは従来どおりクライアントで SQL を実行する。読み取りロールで接続すると、許可されたビューの一覧と照会を利用できる。

実サービスには「サービス接続…」からホスト・ポート・トークンを入力するか、トークンファイルのフルパスを指定する。既定は `127.0.0.1:7400`。認証トークンは設定ファイルへ保存しない。サービス起動は [catdb-service.md](../../docs/catdb-service.md) を参照。

### 組み込みcatdbへの直接接続

- 「DBファイルを開く…」で既存の catdb データベース（*.ndb）を指定する。フルパスを入力するか、「参照…」でファイルを選んで「決定」を押す。
- 同じ画面で `.catlink`（旧名 `catdb-current.txt`）も開ける。リンクには相対パスを1行で記述する。`data/main.ndb` ならその DB を開く。`data` のようにフォルダーを指定すると、書庫のテーブル定義（通常の SQL データベースでは各 DB のカタログ）からテーブル一覧を表示する。左のテーブル名を選ぶと、その列名とデータ行を最大100行ずつ閲覧できる。閲覧には一時コピーを使う。書庫にも catdb の SELECT を実行できる。例: `SELECT * FROM Book_3 WHERE PublishDate IS NOT NULL`。列の選択、絞り込み、並べ替え、集計、JOIN、IN サブクエリに対応する。SQL で比較・集計する際はプレビュー省略前の値を使う。
- 「新規DB…」で新しいファイルを作る（初期ファイル名はcatdb.ndb）。親フォルダーは事前に作成し、既存ファイルは上書きしない。
- トークンや常駐サービスは不要。接続種別とファイル名を左側に、フルパスをステータスに表示する。
- SQLの構文・結果表示・確定・取消はサービス接続と共通。接続方式を変更する前に、未確定トランザクションをCOMMITまたはROLLBACKする。
- 「接続解除」では未確定の変更を破棄し、ファイルの所有権を解放する。再接続すると確定済みのテーブルとデータを復元する。
- 使用中のファイルは二重に開かず、使用中であることをエラー表示する。同じファイルをサービスと組み込みで同時に所有する使い方はしない。

SQL表の保存形式はサービス接続と同じで、接続を閉じてから同じDBを別方式で開ける。生のキー値データをSQL表へ自動変換する機能ではない。SQL表の定義がないトランザクションDBはテーブル一覧が空になり、異なるファイル形式はエラーになる。ファイルを開く際のログ復旧は既存catdbエンジンが行う。

SQLを入力して「▶ 実行」を押す。テーブル一覧は名前順で、クリックするとそのテーブルを検索する。複数クエリタブ、SQLファイルの保存・読込、結果のCSV出力に対応する。ファイル操作はアプリ内でフルパスを指定し、既存ファイルには上書きしない。CSVは取得結果の全行を出力し、画面の表示フィルターとは独立する。複数SQL文を実行した場合は最後の文の結果を表示し、各文の件数・完了状態をメッセージに表示する。

### トランザクション DB の管理

- 「チェックポイント」は組み込み DB または管理者の TCP 接続で実行する。未確定のトランザクションは先に終了する。
- 「キー・値の操作…」は管理者向けの生キー操作。「範囲の値を出力…」は組み込み DB の指定範囲から、値の読み取り用スナップショットを出力する。
- 「バックアップ…」は、開いている組み込み DB の確定状態を新規バックアップに保存する。「新規DBへ復元…」は検証済みのバックアップを新しい DB に復元する。出力先は既存ファイルを指定できない。TCP サーバーのファイルパスを指定して遠隔バックアップを取る機能は含まない。

## 結果の列と長い値

左端の行番号はSQL結果の先頭を1として表示する。絞り込み後も元の結果の行番号を維持する。行番号欄はデータ列とは独立しており、CSVには含めない。行の仮想化にも対応する。

ヘッダーとデータを共通の列幅（初期240px）で区切り、縦横の境界線を表示する。ヘッダー右端の境界を左右へドラッグすると、その列の見出しと全行が同時に広がる・縮まる。境界をダブルクリックすると、見出しと現在表示中の行（表示フィルター適用後）の内容に合わせて自動調整する。境界へキーボードでフォーカスした場合は左右キーで手動調整、Enterで自動調整できる。幅は80～1200px。同じ表の表示フィルター・再選択では幅を保持する。幅の設定はアプリ起動中のみ保持し、アプリ再起動時は初期幅へ戻る。長い値はセル内の1行で末尾を省略し、隣の列へはみ出さない。横に長い表は水平スクロールで確認する。ホバーするとセルの表示値を確認できる。この描画上の省略でデータやCSVの値は変えない。

## 応答性と検証

結果表はWinUI ListView/ItemsStackPanelで行を仮想化し、画面付近の行だけを生成する。絞り込み入力は180msの待機でまとめ、列幅のドラッグ更新は約16ms単位で集約する。

2026-11-09のDebug環境では、1,500行×2列のSQL結果で初期生成7行、更新準備4.0ms、初回レイアウト39.4msを確認。環境・キャッシュ状態で変わる参考値で、改修前との同条件比較や描画フレーム時間を示すものではない。検証記録はartifacts/owner-responsiveness-verification.json。

## SQLの例

```sql
CREATE TABLE books (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    author TEXT,
    price REAL,
    available BOOLEAN DEFAULT TRUE
);

INSERT INTO books (id, title, author, price) VALUES
    (1, 'catdb入門', '猫山 まこと', 1800),
    (2, '小さなストアの設計', NULL, 2500);

SELECT id, title AS 書名, price
FROM books
WHERE price >= 2000 AND author IS NULL
ORDER BY id DESC
LIMIT 100;
```

```sql
BEGIN;
UPDATE books SET price = 2200 WHERE id = 1;
DELETE FROM books WHERE id = 2;
ROLLBACK;
SELECT * FROM books ORDER BY id;
```

BEGIN外では変更を文ごとに自動確定する。複数文を一括で取り消したい場合はBEGINを明示する。BEGIN / COMMIT / ROLLBACKボタンも同じトランザクションを操作する。全SQL文の構文を実行前に検査する。実行時エラーでバッチが途中停止した場合、先行文の確定済み変更は残り、メッセージに実行済みの文数を表示する。

## 対応構文

保存エンジン・TCP サービスと管理アプリの機能を比較するには、[catdb 本体と catdb Owner の SQL 構文マトリックス](../../README.md#sql-syntax-matrix)を参照。

|用途|対応内容|
|---|---|
|テーブル|CREATE TABLE、CREATE TABLE … AS SELECT、DROP TABLE、SHOW TABLES。ALTER TABLE で列の追加・改名・削除、互換性のある型への変更、DEFAULT・NOT NULL の設定と解除。|
|主キー索引|新規の主キー付きテーブルに自動作成。`REINDEX テーブル名` は主キーがあれば主キー索引を作成・再構築し、そのテーブルの一般列の索引もすべて再構築する。|
|一般列の索引|`CREATE INDEX [IF NOT EXISTS] 索引名 ON テーブル名 (列名[, 列名 …])`、`DROP INDEX [IF EXISTS] 索引名 ON テーブル名`。索引名はテーブルごとに管理する。|
|ビュー|CREATE VIEW、DROP VIEW、SHOW VIEWS。単一表の列を直接公開するビューは INSERT・UPDATE・DELETE に対応。|
|検索|SELECT *、列指定、AS、DISTINCT、CASE、CAST、算術式、定数 SELECT。|
|結合|INNER・LEFT・RIGHT・FULL・CROSS・NATURAL JOIN、USING。最大8つの表またはビュー。ON では修飾列同士の等価条件を AND で連結でき、一致候補を追加条件で絞り込める。|
|実行計画|SQLデータベースでは `EXPLAIN SELECT ...` で走査・索引・結合方法を確認できる。`ANALYZE テーブル名` で推定に使う統計を更新する。|
|集計|GROUP BY、HAVING、COUNT(*)、COUNT(列)、COUNT(DISTINCT 列)、SUM・AVG・MIN・MAX。OVER で順位・集計・前後行の値を取得できる。|
|副問い合わせ|IN、EXISTS、スカラー・相関副問い合わせ、FROM の派生表。WITH、WITH RECURSIVE、UNION [ALL]・INTERSECT・EXCEPT に対応。|
|プロシージャー|CREATE PROCEDURE、EXEC / EXECUTE、DROP PROCEDURE。引数なし、本文は最大100文。|
|権限|GRANT / REVOKE でビューの SELECT と、表・ビューの INSERT・UPDATE・DELETE をロールごとに設定できる。|
|条件|比較、AND・OR・NOT、括弧、IS [NOT] NULL、LIKE、BETWEEN、IN (値リスト)。|
|並びと件数|ORDER BY、ASC / DESC、LIMIT / OFFSET、TOP、OFFSET … ROWS FETCH NEXT … ROWS ONLY。|
|挿入|INSERT INTO … VALUES の単一行・複数行と INSERT INTO … SELECT。|
|更新・削除|UPDATE の式による代入、DELETE、MERGE の条件付き更新・削除・挿入。|
|型|INTEGER、REAL、TEXT、BOOLEAN、BLOB|
|制約|単一列・複合 PRIMARY KEY、NOT NULL、UNIQUE、DEFAULT、CHECK、単一列・複合外部キー、ON UPDATE / DELETE CASCADE、TEXT / BLOB の長さ指定。|
|値・名前|文字列の''エスケープ、N'日本語'、NULL、TRUE/FALSE、X'00FF'、[引用名]、二重引用符の識別子|
|トランザクション|BEGIN / START TRANSACTION、COMMIT、ROLLBACK、SAVEPOINT、ROLLBACK TO SAVEPOINT、RELEASE SAVEPOINT。|
|スクリプト|セミコロン区切り、--行コメント、/*ブロックコメント*/|

INT/BIGINT、VARCHAR/NVARCHAR、FLOAT/DOUBLE/DECIMAL、BOOL、VARBINARYは対応する基本型の別名として扱う。INTEGERは符号付き64bit、REALは.NET decimalの範囲で扱う。暗黙の文字列→数値変換は行わない。テーブル名・列名は大文字小文字を区別しない。文字列の比較・LIKEは大文字小文字を区別し、ORDER BYの文字列順はOrdinal。NULLとの通常比較はUNKNOWNとなりWHEREでは選択されない。UNIQUEはNULLを複数許し、PRIMARY KEYはNULLを拒否する。

単一列の外部キーは主キー列または UNIQUE 列を、複合外部キーは複合主キーを参照する。ON UPDATE CASCADE と ON DELETE CASCADE は、参照先の変更を従属行と索引に同じトランザクションで反映する。

```sql
SELECT b.title, a.name AS author
FROM books AS b
INNER JOIN authors AS a ON b.author_id = a.id
ORDER BY a.name, b.title;
```

この例は `books(title, author_id)` と `authors(id, name)` を対象にする。結合キーの NULL は一致しない。外部結合では相手のない側の列に NULL が入る。各 JOIN の中間結果は最大4,096行で、WHERE・LIMIT を適用する前に上限を検査する。

ビュー定義は基底テーブルを参照し、JOIN・集計や射影を照会時点のトランザクションで評価する。定義内で別ビューや IN 副問い合わせを参照する機能はない。ビューの公開列や定義を壊す列変更と、参照先テーブルの削除は事前に拒否する。

一般列には `CREATE INDEX by_author_price ON books (author_id, price);` のように1〜64列の索引を作成できる。先頭列から等価条件で絞り、次の列の範囲条件を使える。例は `WHERE author_id = 42 AND price >= 1000`。対応する型のリテラルとの比較が対象で、取得候補は WHERE で再評価する。旧形式の一般列ハッシュ索引は `REINDEX books;` で順序付き索引に移行できる。索引付き書庫への書き込みには対応版のクライアントを使う。

```sql
CREATE TABLE books (title TEXT, public BOOLEAN, internal_note TEXT);
CREATE VIEW public_books AS SELECT title FROM books WHERE public = TRUE;
SELECT * FROM public_books;
```

たとえば管理者が `GRANT SELECT ON public_books TO reader;` を実行すると、reader として認証された利用者は `SHOW VIEWS` と `SELECT * FROM public_books` を使える。読み取りロールからの直接の表照会・生キー操作・更新は拒否する。ホストが利用者を認証する組み込みアプリでは、`QueryRoleViewAsync(sql, role)` またはビュー名の許可リストを取る `QueryPublishedViewAsync(sql, allowedViews)` を使用する。DB ファイルと管理者トークンは管理者側で保持する。

## 保存方式と上限

組み込み DB では共通の [Catdb.Sql](../../dotnet/Catdb.Sql) が Rust エンジンのトランザクション API を利用する。TCP では認証応答の SQL 能力に応じ、サーバーの SQL ホストまたはクライアントの共通実装が実行する。同じトランザクション DB と既存の Owner SQL カタログを読み書きする。[サービスおよび CLI の起動方法](../../docs/catdb-service.md)を参照。

テーブル定義は `_catdb_owner/sql/v1/table/<name hash>`、行は `_catdb_owner/sql/v1/row/<name hash>/<row id>` にJSONとして保存する。既存の生キー・値を自動でSQL表へ変換したり、削除したりしない。SQLデータの名前空間を生のキー値APIから直接変更しないこと。owner以外のクライアントがSQLを扱う場合も同じ形式と制約処理が必要。

- 256テーブル、1表64列、1表4096行、結果64列。
- 1行・テーブル定義はシリアライズ後256 KiB以下。
- 1回の表走査・1文の変更量は8 MiB以下。
- 1スクリプトは1 MiB・100文以下。
- 検索は索引に対応する等価・範囲条件で候補を取得し、適用できない条件では同じトランザクションでテーブルをページ走査する。上限を超えると部分結果を成功扱いせずエラーにする。

保存前に文全体の型・NULL・一意性を検査する。書込途中でサービスが拒否した場合は、途中の変更が残らないよう現在のトランザクション全体を取り消す。明示BEGIN中の場合は、それ以前の未確定変更も取り消される。検査段階だけのエラーでは明示トランザクションを維持する。確定応答を失った場合は自動再送せず、接続とデータの状態を照合する。

組み込み接続にはサービスの接続期限はなく、明示トランザクションは確定・取消・切断まで保持する。組み込みの資源上限は既存C#ラッパーの標準設定（変更32 MiB、読取依存16 MiB、保持128 MiB、バッファー16 MiB、最大64トランザクション）を使用する。

サービスの未使用接続期限は既定30秒、トランザクション保持期限は既定60秒。期限切れなどで切断された場合は再接続する。通信はTLSを内蔵しない。別ホストへ接続する場合は暗号化トンネルを使う。

## 過去の検証記録

2026-11-09、VS 2026 CommunityでDebugビルド成功、エラー0・警告0。SQLコア40件・実ネイティブ組み込み7件のxUnitテスト、計47件成功。合意済み観点に対する仕様ベースのテストで、型・CRUD・引用符・コメント・NULL・条件の優先順位・並べ替え・件数境界・主キー・既存データ保護・明示/自動トランザクション・書込途中失敗・確定応答喪失時の再送防止を確認した。

```powershell
dotnet test apps/Catdb.Owner.Sql.Tests/Catdb.Owner.Sql.Tests.csproj
```

vs-mcp UIAで、デモの自由列テーブル作成・日本語検索・取消、実サービスのCREATE/INSERT/SELECT/UPDATE/DELETE/COMMIT/ROLLBACK、サーバー再起動後の永続化、重複主キー拒否、任意の結果列、SQL保存・再読込、CSVヘッダー・日本語・絵文字・NULLの復元を確認。専用ポート17431と新規DBを使い、既存DBには接続していない。

追加の実画面検証では、組み込みDB新規作成、SQLで日本語・絵文字を保存、ROLLBACK、未確定INSERT中の切断と再オープン、既存ファイル上書き拒否・非対応ファイル拒否、サービスが使用中のDBの拒否を確認。サービス作成DBを組み込みから読み書きし、その変更を再びサービス接続から読めることも確認した。互換検証の専用サービスは停止済み。

実電源断、大規模データ、SQLクライアント間の並行負荷、配布先クリーン環境、他OS、スクリーンリーダーの通し検証は未実施。

203文字のSQL値がCSVでは省略されず保持されることを確認した。

2026-11-09の列幅変更では、SQLデモでヘッダー境界のドラッグによる拡大・縮小を実画面で確認。見出しとデータの左端・幅が一致し、末尾省略を維持すること、フィルター・空表への切替後に戻っても列幅を保持することをUIAの座標で確認した。表が表示領域より狭い場合も左端を固定する。ビルドはエラー0・警告0。
