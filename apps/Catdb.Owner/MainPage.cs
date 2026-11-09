using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using Catdb.Owner.Services;
using Catdb.Owner.Controls;
using Catdb.Sql;

namespace Catdb.Owner;

public sealed partial class MainPage : ContentPage
{
    private static readonly Color CanvasColor = Color.FromArgb("#10171E");
    private static readonly Color Surface = Color.FromArgb("#18232D");
    private static readonly Color Muted = Color.FromArgb("#94A8B8");
    private static readonly Color Accent = Color.FromArgb("#59D9B3");
    private const string DefaultQuery = "SHOW TABLES;";
    private readonly Editor editor = new() { Text = DefaultQuery, FontFamily = "Consolas", FontSize = 15, IsSpellCheckEnabled = false, IsTextPredictionEnabled = false, TextColor = Color.FromArgb("#C2E8DC"), BackgroundColor = CanvasColor, AutomationId = "QueryEditor" };
    private readonly Label status = Text("未接続 · 接続先を選択してください", 12);
    private readonly Label connection = Text("接続されていません", 13);
    private readonly Label count = Text("0 行", 12);
    private readonly Label timing = Text("待機中", 12);
    private readonly ActivityIndicator busyIndicator = new() { IsVisible = false, Color = Accent, WidthRequest = 18, HeightRequest = 18 };
    private readonly Label transaction = Text("トランザクションなし", 12);
    private readonly Editor messages = new() { IsReadOnly = true, FontFamily = "Consolas", FontSize = 13, BackgroundColor = CanvasColor, TextColor = Muted, AutomationId = "Messages" };
    private readonly ResultTable resultTable = new();
    private readonly VerticalStackLayout explorer = new() { Spacing = 5 };
    private readonly HorizontalStackLayout tabs = new() { Spacing = 2 };
    private readonly Entry host = Input("Host", "127.0.0.1");
    private readonly Entry port = Input("Port", "7400");
    private readonly SecretEntry token = new() { AutomationId = "Token", HeightRequest = 38 };
    private readonly Label connectionError = Text("", 12, color: Color.FromArgb("#FFB4A9"));
    private readonly Entry filter = Input("KeyFilter", "");
    private readonly Grid overlay = new() { BackgroundColor = Color.FromArgb("#D010171E"), IsVisible = false, AutomationId = "ConnectionDialog" };
    private readonly Grid fileOverlay = new() { BackgroundColor = Color.FromArgb("#E010171E"), IsVisible = false };
    private readonly Entry filePath = Input("FilePath", "");
    private readonly Label fileTitle = Text("", 22, true);
    private readonly Label fileError = Text("", 12, color: Color.FromArgb("#FFB4A9"));
    private readonly Label fileHint = Text("", 12, color: Muted);
    private readonly Button browseFile = new() { Text = "参照…", AutomationId = "FileBrowse", MinimumWidthRequest = 84 };
    private TaskCompletionSource<string?>? fileSelection;
    private readonly List<QueryDocument> documents = [new("クエリ 1", DefaultQuery)];
    private SqlResult result = new([], [], "");
    private readonly List<SqlTable> tables = [];
    private SqlDatabase Database => new(SendAsync, () => inTransaction);
    private readonly List<Button> guarded = [];
    private CatdbClient? client;
    private bool IsReadRole => client?.ReadRole is not null;
    private EmbeddedCatdbStore? embedded;
    private CatlinkFolder? folder;
    private FolderTable? folderSelection;
    private long? nextFolderPage;
    private readonly Label sourceNote = Text("SQL テーブル · catdb保存\nCREATE TABLEで作成できます。\n一覧更新: SHOW TABLES;", 12, color: Muted);
    private readonly Label sqlHint = new() { Text = "  SQL · CREATE TABLE / SELECT / INSERT / UPDATE / DELETE · 通常は文ごとに自動確定", FontSize = 11, TextColor = Muted, BackgroundColor = Surface, VerticalTextAlignment = TextAlignment.Center };
    private readonly Button backupButton;
    private readonly Button restoreButton;
    private bool HasConnection => client is not null || embedded is not null;
    private bool inTransaction;
    private int activeDocument;
    private int queryNumber = 1;
    private bool busy;
    private readonly IDispatcherTimer filterTimer;
    private readonly Dictionary<string, double[]> savedColumnWidths = new(StringComparer.Ordinal);



    public MainPage()
    {
        Title = "catdb owner";
        BackgroundColor = CanvasColor;
        filter.Placeholder = "表示済みの結果を絞り込み";
        filterTimer = Dispatcher.CreateTimer();
        filterTimer.Interval = TimeSpan.FromMilliseconds(180);
        filterTimer.IsRepeating = false;
        filterTimer.Tick += (_, _) => RenderRows();
        filter.TextChanged += (_, _) => { filterTimer.Stop(); filterTimer.Start(); };
        timing.AutomationId = "Timing";

        var root = new Grid { RowDefinitions = { new(72), new(52), new(GridLength.Star), new(32) } };
        var brand = new HorizontalStackLayout { Spacing = 14, VerticalOptions = LayoutOptions.Center };
        brand.Add(new Border { BackgroundColor = Accent, StrokeThickness = 0, Padding = new Thickness(10, 6), StrokeShape = new Microsoft.Maui.Controls.Shapes.RoundRectangle { CornerRadius = 8 }, Content = new Label { Text = "c /", FontSize = 23, FontAttributes = FontAttributes.Bold, TextColor = CanvasColor } });
        var brandText = new VerticalStackLayout { Spacing = 2 };
        brandText.Add(Text("catdb owner", 22, true));
        brandText.Add(Text("DATABASE WORKSPACE", 10, color: Muted));
        brand.Add(brandText);
        var header = new Grid { Padding = new Thickness(24, 0), ColumnDefinitions = { new(GridLength.Star), new(GridLength.Auto) } };
        header.Add(brand);
        header.Add(new Label { Text = "LOCAL FIRST   /   v0.1", TextColor = Muted, FontSize = 11, VerticalOptions = LayoutOptions.Center }, 1);
        root.Add(header);

        var tools = new HorizontalStackLayout { Spacing = 8, Padding = new Thickness(20, 6), BackgroundColor = Surface };
        tools.Add(Action("サービス接続…", "Connect", () => { connectionError.Text = ""; overlay.IsVisible = true; host.Focus(); return Task.CompletedTask; }, true));
        tools.Add(Action("DBファイルを開く…", "OpenEmbedded", () => OpenEmbeddedAsync(false)));
        tools.Add(Action("新規DB…", "CreateEmbedded", () => OpenEmbeddedAsync(true)));
        backupButton = Action("バックアップ…", "BackupEmbedded", BackupEmbeddedAsync);
        restoreButton = Action("新規DBへ復元…", "RestoreEmbedded", RestoreEmbeddedAsync);
        tools.Add(backupButton);
        tools.Add(restoreButton);
        rawKvButton = Action("キー・値の操作…", "RawKeyValue", RawKvAsync);
        snapshotButton = Action("範囲の値を出力…", "ExportSnapshot", ExportSnapshotAsync);
        tools.Add(rawKvButton);
        tools.Add(snapshotButton);
        tools.Add(Action("接続解除", "Disconnect", DisconnectAsync));
        tools.Add(Action("＋ 新しいクエリ", "NewQuery", NewQueryAsync));
        tools.Add(Action("開く", "OpenQuery", OpenAsync));
        tools.Add(Action("保存", "SaveQuery", SaveAsync));
        root.Add(new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = tools }, 0, 1);

        var workspace = new Grid { ColumnSpacing = 1, BackgroundColor = Color.FromArgb("#2A3945"), ColumnDefinitions = { new(264), new(GridLength.Star) } };
        var sidebar = new Grid { BackgroundColor = Surface, Padding = new Thickness(18, 20), RowDefinitions = { new(36), new(50), new(GridLength.Star), new(112) } };
        sidebar.Add(Text("オブジェクト エクスプローラー", 13, true));
        sidebar.Add(connection, 0, 1);
        sidebar.Add(new ScrollView { Content = explorer }, 0, 2);
        var note = new VerticalStackLayout { Spacing = 8 };
        note.Add(Text("CATDB · EMBEDDED / SERVICE", 10, true, Muted));
        note.Add(sourceNote);
        sidebar.Add(note, 0, 3);
        workspace.Add(sidebar);

        var pane = new Grid { BackgroundColor = CanvasColor, RowSpacing = 1, RowDefinitions = { new(42), new(48), new(new GridLength(0.9, GridUnitType.Star)), new(32), new(48), new(new GridLength(1.1, GridUnitType.Star)), new(88) } };
        pane.Add(new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = tabs, BackgroundColor = Surface });
        var commandBar = new HorizontalStackLayout { Spacing = 8, Padding = new Thickness(14, 5) };
        commandBar.Add(Action("▶ 実行", "Execute", ExecuteAsync, true));
        commandBar.Add(Action("一覧を取得", "Browse", BrowseAsync));
        commandBar.Add(Action("BEGIN", "Begin", () => TransactionAsync("begin")));
        commandBar.Add(Action("COMMIT", "Commit", () => TransactionAsync("commit")));
        commandBar.Add(Action("ROLLBACK", "Rollback", () => TransactionAsync("rollback")));
        commandBar.Add(Action("チェックポイント", "Checkpoint", CheckpointAsync));
        pane.Add(new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = commandBar }, 0, 1);
        var editorFrame = new Grid { Padding = new Thickness(18, 10), ColumnDefinitions = { new(30), new(GridLength.Star) } };
        editorFrame.Add(new Label { Text = "›", FontSize = 18, TextColor = Muted, Padding = new Thickness(0, 8, 0, 0) });
        editorFrame.Add(editor, 1);
        pane.Add(editorFrame, 0, 2);
        pane.Add(sqlHint, 0, 3);
        var resultsBar = new Grid { Padding = new Thickness(16, 4), ColumnSpacing = 14, ColumnDefinitions = { new(GridLength.Auto), new(GridLength.Auto), new(GridLength.Star), new(GridLength.Auto) } };
        resultsBar.Add(Text("結果", 14, true));
        resultsBar.Add(count, 1);
        resultsBar.Add(filter, 2);
        resultsBar.Add(Action("CSV 出力", "Export", ExportAsync), 3);
        pane.Add(resultsBar, 0, 4);
        pane.Add(resultTable, 0, 5);
        pane.Add(messages, 0, 6);
        workspace.Add(pane, 1);
        root.Add(workspace, 0, 2);
        var footer = new Grid { BackgroundColor = Color.FromArgb("#173D38"), Padding = new Thickness(16, 0), ColumnSpacing = 20, ColumnDefinitions = { new(GridLength.Star), new(GridLength.Auto), new(GridLength.Auto), new(GridLength.Auto) } };
        footer.Add(status);
        footer.Add(transaction, 1);
        footer.Add(timing, 2);
        footer.Add(busyIndicator, 3);
        root.Add(footer, 0, 3);
        var layer = new Grid();
        layer.Add(root);
        BuildConnectionDialog();
        layer.Add(overlay);
        BuildFileDialog();
        layer.Add(fileOverlay);
        Content = layer;
        RenderTabs();
        RenderExplorer();
        RenderRows();
        UpdateMode();
        Log("サービス・DBファイル・新規DBから接続先を選べます。");
        Unloaded += (_, _) => CloseStorage();
        status.LineBreakMode = LineBreakMode.TailTruncation; status.MaxLines = 1;
        connection.LineBreakMode = LineBreakMode.TailTruncation; connection.MaxLines = 1;
    }

    private void BuildConnectionDialog()
    {
        var panel = new VerticalStackLayout { WidthRequest = 460, Spacing = 14, Padding = 28, BackgroundColor = Surface, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center };
        panel.Add(Text("サービスに接続", 24, true));
        panel.Add(Text("catdb-server の接続先を指定してください。", 13, color: Muted));
        panel.Add(Text("ホスト", 12)); panel.Add(host);
        panel.Add(Text("ポート", 12)); panel.Add(port);
        panel.Add(Text("認証トークン", 12)); panel.Add(token);
        panel.Add(Action("トークンファイルを選択…", "TokenFile", async () =>
        {
            var path = await PickPathAsync("トークンファイルを開く", "", false);
            if (path is null) return;
            await using var stream = File.OpenRead(path);
            using var reader = new StreamReader(stream);
            var buffer = new char[4097];
            var length = await reader.ReadBlockAsync(buffer, 0, buffer.Length);
            if (length > 4096) throw new InvalidOperationException("トークンファイルが大きすぎます。");
            token.Text = new string(buffer, 0, length).TrimEnd('\r', '\n');
        }));
        panel.Add(Text("TCP v1 · TLS非対応。別ホストへ接続する場合は\n暗号化トンネルを利用してください。", 12, color: Muted));
        var actions = new HorizontalStackLayout { Spacing = 8 };
        actions.Add(Action("接続する", "ConnectSubmit", ConnectAsync, true));
        actions.Add(Action("キャンセル", "ConnectCancel", () => { token.Text = ""; overlay.IsVisible = false; return Task.CompletedTask; }));
        panel.Add(actions);
        panel.Add(connectionError);
        overlay.Add(panel);
    }

    private async Task ConnectAsync()
    {
        if (inTransaction) throw new InvalidOperationException("接続を変更する前に COMMIT または ROLLBACK してください。");
        if (string.IsNullOrWhiteSpace(host.Text) || !int.TryParse(port.Text, out var number) || number is < 1 or > 65535 || string.IsNullOrWhiteSpace(token.Text))
            throw new InvalidOperationException("ホスト・1～65535のポート・認証トークンを入力してください。");
        var candidate = new CatdbClient();
        try { await candidate.ConnectAsync(host.Text.Trim(), number, token.Text); }
        catch { candidate.Dispose(); throw; }
        CloseStorage(); client = candidate;
        token.Text = "";
        overlay.IsVisible = false;
        connection.Text = $"● {host.Text.Trim()}:{number}";
        ToolTipProperties.SetText(connection, $"サービス · {host.Text.Trim()}:{number}");
        status.Text = $"接続済み · {host.Text.Trim()}:{number}";
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log(candidate.SupportsSql ? "認証に成功しました。SQL は catdb サーバーで実行します。" : "認証に成功しました。SQL は Owner で実行します。");
        if (IsReadRole)
        {
            Log($"閲覧ロール {candidate.ReadRole} で接続しました。");
            editor.Text = "SHOW VIEWS;";
        }
        await RefreshTablesAsync();
    }

    private async Task OpenEmbeddedAsync(bool create)
    {
        if (inTransaction) throw new InvalidOperationException("接続を変更する前に COMMIT または ROLLBACK してください。");
        var path = await PickPathAsync(create ? "組み込みcatdbを新規作成" : "組み込みcatdbを開く", create ? embedded?.DatabasePath ?? Path.Combine(FileSystem.AppDataDirectory, "catdb.ndb") : string.Empty, create, browse: !create);
        if (path is null) return;
        if (!create && (Path.GetExtension(path).Equals(".catlink", StringComparison.OrdinalIgnoreCase)
            || Path.GetFileName(path).Equals("catdb-current.txt", StringComparison.OrdinalIgnoreCase)))
        {
            path = Catlink.Resolve(path);
            if (Directory.Exists(path))
            {
                var loaded = await Task.Run(() => CatlinkFolder.LoadAsync(path));
                CloseStorage(); tables.Clear();
                folder = loaded;
                connection.Text = $"● {Path.GetFileName(path)}";
                ToolTipProperties.SetText(connection, path);
                status.Text = $"フォルダー閲覧 · {path}";
                await BrowseAsync();
                Log($"{loaded.Tables.Count} 件のテーブルを読み込みました。");
                return;
            }
        }
        if (!create && embedded is not null && string.Equals(path, embedded.DatabasePath, StringComparison.OrdinalIgnoreCase))
        { Log("この組み込みDBには既に接続しています。"); return; }
        await OpenEmbeddedPathAsync(path, create);
    }

    private async Task OpenEmbeddedPathAsync(string path, bool create)
    {
        var candidate = await EmbeddedCatdbStore.OpenAsync(path, create);
        CloseStorage(); embedded = candidate;
        connection.Text = $"● 組み込み / {Path.GetFileName(path)}";
        ToolTipProperties.SetText(connection, path);
        status.Text = $"組み込み接続 · {path}";
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log($"組み込みcatdbを{(create ? "作成して" : "直接")}開きました: {path}\nトークンとサービスは不要です。生のキー値はSQLテーブルへ自動変換しません。");
        await RefreshTablesAsync();
    }

    private async Task BackupEmbeddedAsync()
    {
        var store = embedded ?? throw new InvalidOperationException("バックアップするローカルDBを開いてください。");
        var suggested = Path.ChangeExtension(store.DatabasePath, ".backup");
        var path = await PickPathAsync("確定済みのローカルDBをバックアップ", suggested, true);
        if (path is null) return;
        await store.BackupAsync(path);
        Log($"バックアップを保存しました: {path}");
    }

    private async Task RestoreEmbeddedAsync()
    {
        if (inTransaction) throw new InvalidOperationException("復元先を開く前に COMMIT または ROLLBACK してください。");
        var directory = Path.GetDirectoryName(embedded?.DatabasePath) ?? FileSystem.AppDataDirectory;
        var source = await PickPathAsync("復元するバックアップを選択", Path.Combine(directory, "catdb.backup"), false, browse: true);
        if (source is null) return;
        var suggested = Path.Combine(Path.GetDirectoryName(source)!, Path.GetFileNameWithoutExtension(source) + "-restored.ndb");
        var target = await PickPathAsync("復元先の新規DBを指定", suggested, true);
        if (target is null) return;
        await EmbeddedCatdbStore.RestoreAsync(source, target);
        Log($"バックアップから新しいDBを復元しました: {target}");
        await OpenEmbeddedPathAsync(target, false);
    }

    private void UpdateMode()
    {
        foreach (var button in guarded.Where(b => b.AutomationId == "Execute"))
            button.IsEnabled = !busy;
        backupButton.IsEnabled = !busy && embedded is not null;
        restoreButton.IsEnabled = !busy && !inTransaction;
        rawKvButton.IsEnabled = !busy && CanUseRawStore;
        snapshotButton.IsEnabled = !busy && embedded is not null && !IsReadRole;
        rawKvButton.IsVisible = snapshotButton.IsVisible = !IsReadRole;
        foreach (var button in guarded.Where(b => b.AutomationId is "Begin" or "Commit" or "Rollback" or "Checkpoint"))
            button.IsVisible = folder is null && !IsReadRole && (button.AutomationId != "Checkpoint" || client is not null || embedded is not null);
        sqlHint.Text = "  SQL · CREATE TABLE / SELECT / INSERT / UPDATE / DELETE · 通常は文ごとに自動確定";
        sourceNote.Text = "SQL テーブル · catdb保存\nCREATE TABLEで作成できます。\n一覧更新: SHOW TABLES;";
        if (folder is not null)
        {
            sourceNote.Text = "テーブルを選ぶと列名と行を表示します。\nANALYZE の統計は、このフォルダーを閉じるまで有効です。";
            sqlHint.Text = "  フォルダー閲覧 · SHOW TABLES / SELECT / ANALYZE / EXPLAIN / EXPLAIN ANALYZE";
        }
        if (IsReadRole)
        {
            sourceNote.Text = $"閲覧ロール · {client!.ReadRole}\n許可されたビューを表示しています。";
            sqlHint.Text = "  SHOW VIEWS / SELECT · 公開ビューの読み取り専用";
        }
    }

    private void CloseStorage()
    {
        folder?.Dispose(); folder = null; folderSelection = null; nextFolderPage = null;
        embedded?.Dispose(); embedded = null;
        client?.Dispose(); client = null;
    }

    private Task DisconnectAsync()
    {
        var wasEmbedded = embedded is not null;
        CloseStorage(); inTransaction = false;
        connection.Text = "接続されていません"; status.Text = "未接続";
        ToolTipProperties.SetText(connection, "未接続");
        transaction.Text = "トランザクションなし";
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log(wasEmbedded ? "組み込みDBを閉じました。未確定の変更を破棄し、所有権を解放しました。" : "接続を閉じました。未確定の変更を破棄しました。");
        return Task.CompletedTask;
    }

    private async Task ExecuteAsync()
    {
        if (!HasConnection && folder is null) throw new InvalidOperationException("サービスへ接続するか、DBファイルを開いてください。");
        var watch = Stopwatch.StartNew();
        result = new([], [], ""); RenderRows();
        IReadOnlyList<SqlResult> output;
        if (folder is not null)
        {
            var currentFolder = folder;
            var sql = editor.Text;
            output = await Task.Run(() => currentFolder.ExecuteAsync(sql));
        }
        else if (client?.SupportsSql == true) output = await ExecuteServiceSqlAsync(editor.Text);
        else output = await Database.ExecuteAsync(editor.Text);
        result = output[^1];
        filter.Text = "";
        RenderRows();
        timing.Text = $"{watch.Elapsed.TotalMilliseconds:N0} ms";
        foreach (var item in output) Log(item.Message);
        if (folder is not null)
        {
            folderSelection = null; nextFolderPage = null; RenderExplorer();
            return;
        }
        try { await RefreshTablesAsync(); }
        catch (Exception error) { Log($"SQL実行は完了しましたが、テーブル一覧を更新できません: {error.Message}"); }
    }

    private async Task RefreshTablesAsync()
    {
        if (IsReadRole)
        {
            var visible = await ExecuteServiceSqlAsync("SHOW VIEWS");
            tables.Clear();
            foreach (var row in visible.Single().Rows) tables.Add(new(row[0].Display, [], ViewQuery: ""));
            RenderExplorer();
            return;
        }
        var current = await Database.GetTablesAsync();
        var views = await Database.GetViewsAsync();
        tables.Clear(); tables.AddRange(current); tables.AddRange(views); RenderExplorer();
    }

    private async Task<IReadOnlyList<SqlResult>> ExecuteServiceSqlAsync(string sql)
    {
        var response = await SendAsync(new JsonObject { ["op"] = "sql", ["sql"] = sql });
        return response["results"]?.Deserialize<List<SqlResult>>() ?? throw new IOException("SQL の実行結果を読み取れません。");
    }

    private async Task<JsonObject> SendAsync(JsonObject request)
    {
        var op = request["op"]!.ToString();
        try
        {
            var response = embedded is not null ? await embedded.RequestAsync(request) : await client!.RequestAsync(request);
            if (op == "begin") inTransaction = true;
            if (op is "commit" or "rollback") inTransaction = false;
            if (op == "sql") inTransaction = client!.SqlTransactionActive;
            return response;
        }
        catch (CatdbRequestException error)
        {
            if (op == "commit") inTransaction = false;
            if (op == "sql") inTransaction = client!.SqlTransactionActive;
            if (error.Message is "commit_uncertain" or "checkpoint_uncertain" or "reopen_required" or "unauthorized" or "sql_host_failed_outcome_unknown")
            { CloseStorage(); inTransaction = false; connection.Text = "切断 · 再接続が必要"; tables.Clear(); RenderExplorer(); status.Text = "再接続が必要 · 確定結果はデータで照合してください"; }
            throw;
        }
        catch
        {
            CloseStorage(); inTransaction = false;
            connection.Text = "切断 · 再接続が必要"; tables.Clear(); RenderExplorer();
            status.Text = "接続中断 · DBの再オープンまたは再接続が必要です";
            throw;
        }
        finally { transaction.Text = inTransaction ? "● 未確定トランザクション" : "トランザクションなし"; }
    }

    private async Task TransactionAsync(string op)
    {
        if (!HasConnection) throw new InvalidOperationException("先にサービスへ接続するか、DBファイルを開いてください。");
        var response = await SendAsync(new JsonObject { ["op"] = op });
        Log($"{op.ToUpperInvariant()} 完了");
        await RefreshTablesAsync();
    }

    private async Task CheckpointAsync()
    {
        if (embedded is null && client is null) throw new InvalidOperationException("チェックポイントを実行する DB に接続してください。");
        if (inTransaction) throw new InvalidOperationException("先に COMMIT または ROLLBACK してください。");
        var info = await SendAsync(new JsonObject { ["op"] = "checkpoint" });
        Log($"チェックポイントが完了しました（世代 {info["version"]}、{info["records"]} 件）。");
    }

    private Task BrowseAsync()
    {
        if (folder is not null)
        {
            folderSelection = null; nextFolderPage = null; filter.Text = "";
            result = folder.Summary(); RenderRows(); RenderExplorer();
            return Task.CompletedTask;
        }
        editor.Text = tables.Count > 0 ? $"SELECT * FROM {SqlNames.Quote(tables[0].Name)} LIMIT 100;" : IsReadRole ? "SHOW VIEWS;" : "SHOW TABLES;"; return ExecuteAsync();
    }

    private Task NewQueryAsync()
    {
        documents[activeDocument].Text = editor.Text;
        var query = tables.Count > 0 ? $"SELECT * FROM {SqlNames.Quote(tables[0].Name)} LIMIT 100;" : DefaultQuery;
        documents.Add(new($"クエリ {++queryNumber}", query));
        activeDocument = documents.Count - 1; editor.Text = query; RenderTabs();
        return Task.CompletedTask;
    }

    private void RenderTabs()
    {
        foreach (var button in tabs.Children.OfType<Button>()) guarded.Remove(button);
        tabs.Clear();
        for (var i = 0; i < documents.Count; i++)
        {
            var index = i;
            var tab = Action($"  {documents[i].Title}  ", $"QueryTab{i}", () =>
            {
                documents[activeDocument].Text = editor.Text;
                activeDocument = index; editor.Text = documents[index].Text; RenderTabs();
                return Task.CompletedTask;
            });
            tab.BackgroundColor = i == activeDocument ? CanvasColor : Surface;
            tab.TextColor = i == activeDocument ? Accent : Muted;
            tabs.Add(tab);
        }
    }

    private async Task OpenAsync()
    {
        var path = await PickPathAsync("クエリを開く", Path.Combine(FileSystem.AppDataDirectory, "catdb-query.sql"), false);
        if (path is null) return;
        await using var stream = File.OpenRead(path);
        if (stream.CanSeek && stream.Length > 1024 * 1024) throw new InvalidOperationException("クエリファイルは1 MiB以下にしてください。");
        using var reader = new StreamReader(stream);
        var text = await reader.ReadToEndAsync();
        await NewQueryAsync(); documents[activeDocument].Title = Path.GetFileName(path); editor.Text = text; RenderTabs();
    }

    private async Task SaveAsync()
    {
        var path = await PickPathAsync("クエリを保存", Path.Combine(FileSystem.AppDataDirectory, "catdb-query.sql"), true);
        if (path is null) return;
        await WriteNewFileAsync(path, editor.Text, new UTF8Encoding(false));
        documents[activeDocument].Title = Path.GetFileName(path); RenderTabs();
        Log($"クエリを保存しました: {path}");
    }

    private async Task ExportAsync()
    {
        if (result.Columns.Count == 0) throw new InvalidOperationException("出力する結果がありません。");
        var path = await PickPathAsync("結果をCSVに保存", Path.Combine(FileSystem.AppDataDirectory, "catdb-results.csv"), true);
        if (path is null) return;
        static string Quote(string value) => "\"" + value.Replace("\"", "\"\"") + "\"";
        var csv = new StringBuilder(string.Join(",", result.Columns.Select(Quote)) + "\r\n");
        foreach (var row in result.Rows) csv.AppendLine(string.Join(",", row.Select(x => Quote(x.Display))));
        await WriteNewFileAsync(path, csv.ToString(), new UTF8Encoding(true));
        Log($"取得した{result.Rows.Count}行をCSVへ出力しました: {path}");
    }

    private void BuildFileDialog()
    {
        var panel = new VerticalStackLayout { WidthRequest = 600, Padding = 28, Spacing = 16, BackgroundColor = Surface, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center };
        panel.Add(fileTitle);
        panel.Add(Text("ファイルのフルパス", 13));
        var pathRow = new Grid { ColumnSpacing = 8, ColumnDefinitions = { new(GridLength.Star), new(GridLength.Auto) } };
        pathRow.Add(filePath); pathRow.Add(browseFile, 1);
        panel.Add(pathRow);
        panel.Add(fileHint);
        browseFile.Clicked += async (_, _) =>
        {
            browseFile.IsEnabled = false;
            fileError.Text = "";
            try
            {
                var window = Window?.Handler?.PlatformView as Microsoft.UI.Xaml.Window
                    ?? throw new InvalidOperationException("ファイル選択画面を開けませんでした。フルパスを入力してください。");
                var selected = await DatabaseFilePicker.PickAsync(window, filePath.Text ?? "", fileTitle.Text);
                if (selected is not null) filePath.Text = selected;
            }
            catch (Exception error) { fileError.Text = $"ファイルを選択できませんでした: {error.Message}"; }
            finally { browseFile.IsEnabled = true; }
        };
        var buttons = new HorizontalStackLayout { Spacing = 8 };
        var submit = new Button { Text = "決定", AutomationId = "FileSubmit", BackgroundColor = Accent, TextColor = CanvasColor };
        submit.Clicked += (_, _) =>
        {
            try
            {
                if (string.IsNullOrWhiteSpace(filePath.Text) || !Path.IsPathFullyQualified(filePath.Text))
                    throw new InvalidOperationException("ファイルのフルパスを入力してください。");
                var path = Path.GetFullPath(filePath.Text.Trim());
                if (savingFile && File.Exists(path)) throw new InvalidOperationException("同名のファイルが存在します。別の名前を指定してください。");
                if (!savingFile && !File.Exists(path)) throw new InvalidOperationException("ファイルが見つかりません。");
                if (!Directory.Exists(Path.GetDirectoryName(path))) throw new InvalidOperationException("保存先のフォルダーが見つかりません。");
                fileOverlay.IsVisible = false; fileSelection?.TrySetResult(path);
            }
            catch (Exception error) { fileError.Text = error.Message; }
        };
        var cancel = new Button { Text = "キャンセル", AutomationId = "FileCancel" };
        cancel.Clicked += (_, _) => { fileOverlay.IsVisible = false; fileSelection?.TrySetResult(null); };
        buttons.Add(submit); buttons.Add(cancel); panel.Add(buttons); panel.Add(fileError); fileOverlay.Add(panel);
    }

    private bool savingFile;
    private static async Task WriteNewFileAsync(string path, string contents, Encoding encoding)
    {
        await using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None, 4096, true);
        await using var writer = new StreamWriter(stream, encoding);
        await writer.WriteAsync(contents);
    }

    private Task<string?> PickPathAsync(string title, string path, bool saving, bool browse = false)
    {
        savingFile = saving; fileTitle.Text = title; filePath.Text = path; fileError.Text = "";
        browseFile.IsVisible = browse;
        fileHint.Text = saving ? "保存先のフォルダーは事前に作成してください。\n既存ファイルへの上書きは実行しません。別の名前で保存してください。"
            : browse ? "参照から既存ファイルを選択するか、フルパスを入力してください。" : "既存ファイルのフルパスを入力してください。";
        fileSelection = new(TaskCreationOptions.RunContinuationsAsynchronously);
        fileOverlay.IsVisible = true; filePath.Focus();
        return fileSelection.Task;
    }

    private void RenderExplorer()
    {
        foreach (var button in explorer.Children.OfType<Button>()) guarded.Remove(button);
        explorer.Clear();
        if (folder is not null)
        {
            explorer.Add(Text($"テーブル ({folder.Tables.Count})", 13, true));
            if (folderSelection is not null && nextFolderPage is not null)
                explorer.Add(Action("次のページ", "FolderNext", () => ShowFolderPageAsync(folderSelection, nextFolderPage.Value)));
            foreach (var table in folder.Tables)
                explorer.Add(TableAction(folder.Label(table), $"FolderTable{explorer.Count}", () => ShowFolderPageAsync(table, 0)));
            return;
        }
        explorer.Add(Text(embedded is not null ? "⌄  組み込みデータベース" : client is null ? "接続先なし" : "⌄  サービス データベース", 13, true));
        if (!HasConnection) return;
        explorer.Add(Action("テーブル・ビュー一覧を更新", "RefreshTables", async () => { await RefreshTablesAsync(); Log($"一覧を更新しました（{tables.Count} 件）"); }));
        explorer.Add(Text($"テーブル・ビュー ({tables.Count})", 11, color: Muted));
        foreach (var table in tables)
        {
            var caption = table.ViewQuery is null ? $"▦  {table.Name}" : $"VIEW  {table.Name}";
            explorer.Add(TableAction(caption, $"Table{SqlNames.Hex(table.Name)}", () => { editor.Text = $"SELECT * FROM {SqlNames.Quote(table.Name)} LIMIT 100;"; return ExecuteAsync(); }));
            foreach (var column in table.Columns)
                explorer.Add(Text($"    {column.Name}  {column.Type}{(column.PrimaryKey ? "  PK" : column.NotNull ? "  NOT NULL" : "")}", 11, color: Muted));
            foreach (var index in table.Indexes ?? [])
                explorer.Add(Text($"    INDEX  {index.Name} ({string.Join(", ", index.ColumnNames)})", 11, color: Muted));
        }
    }

    private async Task ShowFolderPageAsync(FolderTable table, long offset)
    {
        if (folder is null) return;
        var page = await Task.Run(() => folder.ReadPageAsync(table, offset));
        folderSelection = table; nextFolderPage = page.Next;
        status.Text = $"フォルダー閲覧 · {table.Name}";
        filter.Text = ""; result = page.Result; RenderRows(); RenderExplorer();
    }

    private Button TableAction(string caption, string id, Func<Task> action)
    {
        var button = Action(caption, id, action);
        button.HorizontalOptions = LayoutOptions.Fill;
        button.HandlerChanged += (_, _) =>
        {
            if (button.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.Button native)
                native.HorizontalContentAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Left;
        };
        return button;
    }

    private void RenderRows()
    {
        filterTimer.Stop();
        var watch = Stopwatch.StartNew();
        var source = embedded is not null ? $"DB:{embedded.DatabasePath}" : connection.Text;
        var layoutKey = JsonSerializer.Serialize(new { Source = source, Columns = result.Columns });
        if (!savedColumnWidths.TryGetValue(layoutKey, out var widths))
            savedColumnWidths[layoutKey] = widths = Enumerable.Repeat(240d, result.Columns.Count).ToArray();
        var query = filter.Text ?? "";
        var visible = result.Rows.Select((row, index) => (Row: row, Number: (long)index + 1))
            .Where(item => query.Length == 0 || item.Row.Any(x => x.Display.Contains(query, StringComparison.OrdinalIgnoreCase))).ToArray();
        count.Text = $"{visible.Length} / {result.Rows.Count} 行";
        var emptyText = result.Rows.Count == 0 ? "結果は0行です。" : "条件に一致する行がありません。";
        resultTable.SetData(result.Columns, visible.Select(item => item.Row.Select(x => x.Display).ToArray()).ToArray(), widths, emptyText, visible.Select(item => item.Number).ToArray());
        Debug.WriteLine($"RenderRows: rows={visible.Length}, prepare/apply={watch.Elapsed.TotalMilliseconds:F1}ms");
    }
    private Button Action(string caption, string id, Func<Task> action, bool primary = false)
    {
        var button = new Button { Text = caption, AutomationId = id, FontSize = 12, CornerRadius = 5, Padding = new Thickness(12, 6), BackgroundColor = primary ? Accent : Surface, TextColor = primary ? CanvasColor : Colors.White, MinimumHeightRequest = 34 };
        SemanticProperties.SetDescription(button, caption);
        guarded.Add(button);
        button.Clicked += async (_, _) =>
        {
            if (busy) return;
            busy = true;
            busyIndicator.IsVisible = true; busyIndicator.IsRunning = true;
            foreach (var item in guarded) item.IsEnabled = false;
            try { await action(); }
            catch (Exception error)
            {
                Log($"エラー: {error.Message}");
                if (overlay.IsVisible) connectionError.Text = error.Message;
                timing.Text = "エラー · メッセージを確認";
            }
            finally { busy = false; busyIndicator.IsRunning = false; busyIndicator.IsVisible = false; foreach (var item in guarded) item.IsEnabled = true; UpdateMode(); }
        };
        return button;
    }

    private void Log(string message)
    {
        var text = $"{DateTime.Now:HH:mm:ss}  {message}\n{messages.Text}";
        messages.Text = text[..Math.Min(12000, text.Length)];
    }
    private static Label Text(string value, double size, bool bold = false, Color? color = null) => new() { Text = value, FontSize = size, TextColor = color ?? Colors.White, FontAttributes = bold ? FontAttributes.Bold : FontAttributes.None, VerticalTextAlignment = TextAlignment.Center };
    private static Entry Input(string id, string value) => new() { AutomationId = id, Text = value, FontSize = 13, TextColor = Colors.White, BackgroundColor = CanvasColor, MinimumHeightRequest = 36 };
    private sealed class QueryDocument(string title, string text) { public string Title = title; public string Text = text; }
}
