using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using SilverHamu.Owner.Services;
using SilverHamu.Owner.Controls;
using SilverHamu.Sql;

namespace SilverHamu.Owner;

public sealed partial class MainPage : ContentPage
{
    private static readonly Color CanvasColor = Color.FromArgb("#10171E");
    private static readonly Color Surface = Color.FromArgb("#18232D");
    private static readonly Color Muted = Color.FromArgb("#94A8B8");
    private static readonly Color Accent = Color.FromArgb("#59D9B3");
    private const string DefaultQuery = "SHOW TABLES;";
    private readonly Editor editor = new() { Text = DefaultQuery, FontFamily = "Consolas", FontSize = 15, IsSpellCheckEnabled = false, IsTextPredictionEnabled = false, TextColor = Color.FromArgb("#C2E8DC"), BackgroundColor = CanvasColor, AutomationId = "QueryEditor" };
    private SqlCompletionController? completionController;
    private readonly Label status = LocalizedText("DisconnectedSelectAConnection", 12);
    private readonly Label connection = LocalizedText("NotConnected", 13);
    private readonly Label count = LocalizedText("Rows", 12);
    private readonly Label timing = LocalizedText("Ready", 12);
    private readonly LoadingAnimation busyAnimation = new() { WidthRequest = 160, HeightRequest = 160, Aspect = Aspect.AspectFit, AutomationId = "BusyAnimation" };
    private readonly Grid busyIndicator = new() { IsVisible = false, BackgroundColor = Color.FromArgb("#8010171E"), AutomationId = "BusyIndicator" };
    private readonly Label transaction = LocalizedText("NoTransaction", 12);
    private readonly Editor messages = new() { IsReadOnly = true, FontFamily = "Consolas", FontSize = 13, BackgroundColor = CanvasColor, TextColor = Muted, AutomationId = "Messages" };
    private readonly ResultTable resultTable = new();
    private readonly VerticalStackLayout explorer = new() { Spacing = 5 };
    private readonly HorizontalStackLayout tabs = new() { Spacing = 2 };
    private readonly List<Button> queryTabButtons = [];
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
    private readonly Button browseFile = new Button { AutomationId = "FileBrowse", MinimumWidthRequest = 84 }.Localized(Button.TextProperty, "Browse").WithActionIcon();
    private TaskCompletionSource<string?>? fileSelection;
    private readonly List<QueryDocument> documents = [new("", DefaultQuery)];
    private SqlResult result = new([], [], "");
    private readonly List<SqlTable> tables = [];
    private SqlDatabase Database => new(SendAsync, () => embedded?.InTransaction ?? inTransaction);
    private readonly List<Button> guarded = [];
    private ServiceConnection? client;
    private bool IsReadRole => client?.ReadRole is not null;
    private EmbeddedSilverHamuStore? embedded;
    private CatlinkFolder? folder;
    private LoadedQueryResult? loadedResult;
    private long resultPage;
    private readonly Label pagePosition = Text("", 12);
    private readonly Button previousPage;
    private readonly Button nextPage;
    private readonly Button lastPage;
    private readonly Button currentPage;
    private bool pagePickerOpen;
    private bool queryClosePromptOpen;
    private readonly Label sqlHint = new Label { FontSize = 11, TextColor = Muted, BackgroundColor = Surface, VerticalTextAlignment = TextAlignment.Center }.Localized(Label.TextProperty, "SqlEditorHint");
    private readonly Button backupButton;
    private readonly Button restoreButton;
    private bool HasConnection => client is not null || embedded is not null || folder is not null;
    private bool inTransaction;
    private int activeDocument;
    private int queryNumber = 1;
    private bool busy;
    private readonly IDispatcherTimer filterTimer;
    private readonly Dictionary<string, double[]> savedColumnWidths = new(StringComparer.Ordinal);
    private Grid workspacePane = null!;
    private View queryCommands = null!, editorPanel = null!, resultHeaderPanel = null!;
    private int connectionVersion;



    public MainPage()
    {
        Title = "SilverHamu Owner";
        BackgroundColor = CanvasColor;
        filter.Placeholder = L.Get("FilterDisplayedResults");
        filterTimer = Dispatcher.CreateTimer();
        filterTimer.Interval = TimeSpan.FromMilliseconds(180);
        filterTimer.IsRepeating = false;
        editScriptTimer = Dispatcher.CreateTimer();
        editScriptTimer.Interval = TimeSpan.FromMilliseconds(220);
        editScriptTimer.IsRepeating = false;
        editScriptTimer.Tick += (_, _) => SynchronizeEditScript();
        editPreviewStatus.AutomationId = "EditPreviewStatus";
        filterTimer.Tick += (_, _) => RenderRows();
        filter.TextChanged += (_, _) => { filterTimer.Stop(); filterTimer.Start(); };
        timing.AutomationId = "Timing";

        var root = new Grid { RowDefinitions = { new(72), new(52), new(GridLength.Star), new(32) } };
        var brand = new HorizontalStackLayout { Spacing = 14, VerticalOptions = LayoutOptions.Center };
        var brandImage = new Image { Source = "silverhamu.png", WidthRequest = 44, HeightRequest = 44 };
        LocalizationBindings.Bind(brandImage, SemanticProperties.DescriptionProperty, "SilverHamuGinhamuInProfile");
        brand.Add(brandImage);
        var brandText = new VerticalStackLayout { Spacing = 2 };
        brandText.Add(Text("SilverHamu Owner", 22, true));
        brandText.Add(Text("DATABASE WORKSPACE", 10, color: Muted));
        brand.Add(brandText);
        var busyContent = new VerticalStackLayout { Spacing = 10, Padding = 24, WidthRequest = 224 };
        busyContent.Add(busyAnimation);
        var busyLabel = LocalizedText("Working", 14, true);
        busyLabel.HorizontalTextAlignment = TextAlignment.Center;
        busyLabel.AutomationId = "BusyStatus";
        busyContent.Add(busyLabel);
        busyIndicator.Add(new Border { Content = busyContent, BackgroundColor = Surface, Stroke = Color.FromArgb("#334752"), StrokeShape = new Microsoft.Maui.Controls.Shapes.RoundRectangle { CornerRadius = 16 }, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center });
        LocalizationBindings.Bind(busyAnimation, SemanticProperties.DescriptionProperty, "ASilverHamsterRunningInAWheel");
        var header = new Grid { Padding = new Thickness(24, 0), ColumnDefinitions = { new(GridLength.Star), new(GridLength.Auto) } };
        header.Add(brand);
        var headerActions = new HorizontalStackLayout { Spacing = 16, VerticalOptions = LayoutOptions.Center };
        headerActions.Add(new Label { Text = "LOCAL FIRST   /   v0.2.0", TextColor = Muted, FontSize = 11, VerticalOptions = LayoutOptions.Center });
        headerActions.Add(LocalizedAction("Settings", "Settings", () => { settingsOverlay.IsVisible = true; return Task.CompletedTask; }));
        header.Add(headerActions, 1);
        root.Add(header);

        var tools = new HorizontalStackLayout { Spacing = 8, Padding = new Thickness(20, 6), BackgroundColor = Surface };
        tools.Add(LocalizedAction("ConnectToService", "Connect", () => { connectionError.Text = ""; overlay.IsVisible = true; host.Focus(); return Task.CompletedTask; }, true));
        tools.Add(LocalizedAction("OpenDBFile", "OpenEmbedded", () => OpenEmbeddedAsync(false)));
        tools.Add(LocalizedAction("NewDB", "CreateEmbedded", () => OpenEmbeddedAsync(true)));
        backupButton = LocalizedAction("Backup", "BackupEmbedded", BackupEmbeddedAsync);
        restoreButton = LocalizedAction("RestoreToNewDB", "RestoreEmbedded", RestoreEmbeddedAsync);
        tools.Add(backupButton);
        tools.Add(restoreButton);
        rawKvButton = LocalizedAction("KeyValueTools", "RawKeyValue", RawKvAsync);
        snapshotButton = LocalizedAction("ExportValuesInRange", "ExportSnapshot", ExportSnapshotAsync);
        tools.Add(rawKvButton);
        tools.Add(snapshotButton);
        tools.Add(LocalizedAction("Disconnect", "Disconnect", DisconnectAsync));
        tools.Add(LocalizedAction("Open", "OpenQuery", OpenAsync));
        tools.Add(LocalizedAction("Save", "SaveQuery", SaveAsync));
        root.Add(new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = tools }, 0, 1);

        var workspace = new Grid { ColumnSpacing = 1, BackgroundColor = Color.FromArgb("#2A3945"), ColumnDefinitions = { new(264), new(GridLength.Star) } };
        var sidebar = new Grid { BackgroundColor = Surface, Padding = new Thickness(18, 20), RowDefinitions = { new(36), new(50), new(GridLength.Star) } };
        sidebar.Add(LocalizedText("ObjectExplorer", 13, true));
        sidebar.Add(connection, 0, 1);
        sidebar.Add(new ScrollView { Content = explorer }, 0, 2);
        workspace.Add(sidebar);

        var pane = new Grid { BackgroundColor = CanvasColor, RowSpacing = 1, RowDefinitions = { new(42), new(48), new(new GridLength(0.9, GridUnitType.Star)), new(32), new(92), new(new GridLength(1.1, GridUnitType.Star)), new(88) } };
        workspacePane = pane;
        pane.Add(new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = tabs, BackgroundColor = Surface });
        var commandBar = new HorizontalStackLayout { Spacing = 8, Padding = new Thickness(14, 5) };
        commandBar.Add(LocalizedAction("Run", "Execute", ExecuteAsync, true));
        var completeSql = new Button { AutomationId = "CompleteSql", FontSize = 12, BackgroundColor = Surface, TextColor = Colors.White }.Localized(Button.TextProperty, "CompleteCtrlSpace").WithActionIcon();
        completeSql.Clicked += (_, _) => { if (!busy) completionController?.Show(); };
        guarded.Add(completeSql);
        commandBar.Add(completeSql);
        commandBar.Add(LocalizedAction("BrowseObjects", "Browse", BrowseAsync));
        commandBar.Add(LocalizedAction("ReloadEditResults", "ReloadEditResults", ReloadEditAsync));
        commandBar.Add(LocalizedAction("UndoEdit", "UndoEdit", () => ChangeEditHistoryAsync(false)));
        commandBar.Add(LocalizedAction("RedoEdit", "RedoEdit", () => ChangeEditHistoryAsync(true)));
        commandBar.Add(editPreviewStatus);
        commandBar.Add(Action("BEGIN", "Begin", () => TransactionAsync("begin")));
        commandBar.Add(Action("COMMIT", "Commit", () => TransactionAsync("commit")));
        commandBar.Add(Action("ROLLBACK", "Rollback", () => TransactionAsync("rollback")));
        commandBar.Add(LocalizedAction("Checkpoint", "Checkpoint", CheckpointAsync));
        queryCommands = new ScrollView { Orientation = ScrollOrientation.Horizontal, Content = commandBar };
        pane.Add(queryCommands, 0, 1);
        var editorFrame = new Grid { Padding = new Thickness(18, 10), ColumnDefinitions = { new(30), new(GridLength.Star) } };
        editorFrame.Add(new Label { Text = "›", FontSize = 18, TextColor = Muted, Padding = new Thickness(0, 8, 0, 0) });
        editorFrame.Add(editor, 1);
        editorPanel = editorFrame;
        pane.Add(editorFrame, 0, 2);
        pane.Add(sqlHint, 0, 3);
        var resultsBar = new Grid { Padding = new Thickness(16, 4), ColumnSpacing = 14, ColumnDefinitions = { new(GridLength.Auto), new(GridLength.Auto), new(GridLength.Star), new(GridLength.Auto) } };
        resultsBar.Add(LocalizedText("Results", 14, true));
        resultsBar.Add(count, 1);
        resultsBar.Add(filter, 2);
        resultsBar.Add(LocalizedAction("ExportCSV", "Export", ExportAsync), 3);
        previousPage = LocalizedAction("Previous", "ResultPrevious", () => ShowLoadedPageAsync(resultPage - 1));
        nextPage = LocalizedAction("Next", "ResultNext", () => ShowLoadedPageAsync(resultPage + 1));
        lastPage = LocalizedAction("LastPage", "ResultLast", () => ShowLoadedPageAsync(loadedResult!.PageCount - 1));
        currentPage = new Button { Text = "1", AutomationId = "ResultCurrentPage", FontSize = 12, CornerRadius = 5, Padding = new Thickness(12, 6), BackgroundColor = Surface, TextColor = Colors.White, MinimumHeightRequest = 34 };
        currentPage.Clicked += async (_, _) =>
        {
            try { await PickResultPageAsync(); }
            catch (Exception error) { Log(L.Get("CouldNotOpenPageSelection", error.Message)); }
        };
        guarded.Add(currentPage);
        currentPage.MinimumWidthRequest = 42;
        LocalizationBindings.Bind(currentPage, ToolTipProperties.TextProperty, "ChooseOrEnterAPageNumber");
        pagePosition.AutomationId = "ResultPagePosition";
        var paging = new HorizontalStackLayout { Spacing = 8, Padding = new Thickness(16, 0) };
        paging.Add(previousPage); paging.Add(nextPage); paging.Add(lastPage); paging.Add(currentPage); paging.Add(pagePosition);
        var resultHeader = new VerticalStackLayout { Spacing = 0 };
        resultHeader.Add(resultsBar); resultHeader.Add(paging);
        resultHeaderPanel = resultHeader;
        pane.Add(resultHeader, 0, 4);
        pane.Add(resultTable, 0, 5);
        pane.Add(messages, 0, 6);
        workspace.Add(pane, 1);
        root.Add(workspace, 0, 2);
        var footer = new Grid { BackgroundColor = Color.FromArgb("#173D38"), Padding = new Thickness(16, 0), ColumnSpacing = 20, ColumnDefinitions = { new(GridLength.Star), new(GridLength.Auto), new(GridLength.Auto) } };
        footer.Add(status);
        footer.Add(transaction, 1);
        footer.Add(timing, 2);
        root.Add(footer, 0, 3);
        var layer = new Grid();
        layer.Add(root);
        BuildConnectionDialog();
        layer.Add(overlay);
        BuildFileDialog();
        layer.Add(fileOverlay);
        BuildCsvDialog();
        layer.Add(csvOverlay);
        BuildSettingsDialog();
        layer.Add(settingsOverlay);
        layer.Add(busyIndicator);
        fileOverlay.PropertyChanged += (_, args) => { if (args.PropertyName == nameof(IsVisible)) UpdateBusyIndicator(); };
        csvOverlay.PropertyChanged += (_, args) => { if (args.PropertyName == nameof(IsVisible)) UpdateBusyIndicator(); };
        Content = layer;
        settingsOverlay.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(IsVisible)) root.IsEnabled = !settingsOverlay.IsVisible;
        };
        resultTable.AddRowRequested = () => _ = RunActionAsync(AddEditRowAsync);
        resultTable.DuplicateRowsRequested = rows =>
        {
            var selected = rows.Select(row => visibleEditRows[row]).ToArray();
            _ = RunActionAsync(() => DuplicateEditRowsAsync(selected));
        };
        resultTable.DeleteRowsRequested = rows =>
        {
            var selected = rows.Select(row => visibleEditRows[row]).ToArray();
            _ = RunActionAsync(() => DeleteEditRowsAsync(selected));
        };
        resultTable.ChangeHistory = redo =>
        {
            if (documents[activeDocument].Edit is not null) _ = RunActionAsync(() => ChangeEditHistoryAsync(redo));
        };
        resultTable.ReadEditValue = ReadResultEditValue;
        resultTable.PendingEditChanged += (_, _) => UpdateMode();
        resultTable.EditCompleted += (_, _) =>
        {
            if (documents[activeDocument].Edit is { } edit) result = edit.Result;
            RenderRows(); UpdateMode();
        };
        resultTable.EditRequested += OnResultEditRequested;
        RenderTabs();
        RenderExplorer();
        RenderRows();
        UpdateMode();
        Log(L.Get("ConnectToAServiceOpenADBFile"));
        editor.HandlerChanged += (_, _) => AttachCompletion();
        editor.HandlerChanging += (_, _) => { completionController?.Dispose(); completionController = null; };
        AttachCompletion();
        RefreshLanguage();
        editor.TextChanged += OnEditScriptChanged;
        Unloaded += (_, _) => { completionController?.Dispose(); completionController = null; CloseStorage(); };
        status.LineBreakMode = LineBreakMode.TailTruncation; status.MaxLines = 1;
        connection.LineBreakMode = LineBreakMode.TailTruncation; connection.MaxLines = 1;
    }

    private void BuildConnectionDialog()
    {
        var panel = new VerticalStackLayout { WidthRequest = 460, Spacing = 14, Padding = 28, BackgroundColor = Surface, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center };
        panel.Add(LocalizedText("ConnectionTitle", 24, true));
        panel.Add(LocalizedText("EnterTheSilverhamuServerConnectionDetails", 13, color: Muted));
        panel.Add(LocalizedText("Host", 12)); panel.Add(host);
        panel.Add(LocalizedText("Port", 12)); panel.Add(port);
        panel.Add(LocalizedText("AuthenticationToken", 12)); panel.Add(token);
        panel.Add(LocalizedAction("ChooseTokenFile", "TokenFile", async () =>
        {
            var path = await PickPathAsync(L.Get("OpenTokenFile"), "", false);
            if (path is null) return;
            await using var stream = File.OpenRead(path);
            using var reader = new StreamReader(stream);
            var buffer = new char[4097];
            var length = await reader.ReadBlockAsync(buffer, 0, buffer.Length);
            if (length > 4096) throw new InvalidOperationException(L.Get("TheTokenFileIsTooLarge"));
            token.Text = new string(buffer, 0, length).TrimEnd('\r', '\n');
        }));
        panel.Add(LocalizedText("TCPVTLSIsNotSupportedUseAn", 12, color: Muted));
        var actions = new HorizontalStackLayout { Spacing = 8 };
        actions.Add(LocalizedAction("Connect", "ConnectSubmit", ConnectAsync, true));
        actions.Add(LocalizedAction("Cancel", "ConnectCancel", () => { token.Text = ""; overlay.IsVisible = false; return Task.CompletedTask; }));
        panel.Add(actions);
        panel.Add(connectionError);
        overlay.Add(panel);
    }

    private async Task ConnectAsync()
    {
        if (inTransaction) throw new InvalidOperationException(L.Get("COMMITOrROLLBACKBeforeChangingTheConnection"));
        if (string.IsNullOrWhiteSpace(host.Text) || !int.TryParse(port.Text, out var number) || number is < 1 or > 65535 || string.IsNullOrWhiteSpace(token.Text))
            throw new InvalidOperationException(L.Get("EnterAHostAPortFromToAnd"));
        var candidate = new ServiceConnection();
        try { await candidate.ConnectAsync(host.Text.Trim(), number, token.Text); }
        catch { candidate.Dispose(); throw; }
        CloseStorage(); client = candidate;
        token.Text = "";
        overlay.IsVisible = false;
        connection.RemoveBinding(Label.TextProperty);
        connection.Text = $"● {host.Text.Trim()}:{number}";
        ToolTipProperties.SetText(connection, L.Get("Service", host.Text.Trim(), number));
        LocalizationBindings.Bind(status, Label.TextProperty, "Connected", host.Text.Trim(), number);
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log(candidate.SupportsSql ? L.Get("AuthenticatedSQLRunsOnTheSilverhamuServer") : L.Get("AuthenticatedSQLRunsInOwner"));
        if (IsReadRole)
        {
            Log(L.Get("ConnectedWithReadRole", candidate.ReadRole));
            editor.Text = "SHOW VIEWS;";
        }
        await RefreshTablesAsync();
    }

    private async Task OpenEmbeddedAsync(bool create)
    {
        if (inTransaction) throw new InvalidOperationException(L.Get("COMMITOrROLLBACKBeforeChangingTheConnection"));
        var path = await PickPathAsync(create ? L.Get("CreateEmbeddedSilverHamu") : L.Get("OpenEmbeddedSilverHamu"), create ? embedded?.DatabasePath ?? Path.Combine(FileSystem.AppDataDirectory, "silverhamu.ndb") : string.Empty, create, browse: !create);
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
                connection.RemoveBinding(Label.TextProperty);
                connection.Text = $"● {Path.GetFileName(path)}";
                ToolTipProperties.SetText(connection, path);
                LocalizationBindings.Bind(status, Label.TextProperty, "FolderBrowsing", path);
                await BrowseAsync();
                Log(L.Get("LoadedTables", loaded.Tables.Count));
                return;
            }
        }
        if (!create && embedded is not null && string.Equals(path, embedded.DatabasePath, StringComparison.OrdinalIgnoreCase))
        { Log(L.Get("ThisEmbeddedDBIsAlreadyConnected")); return; }
        await OpenEmbeddedPathAsync(path, create);
    }

    private async Task OpenEmbeddedPathAsync(string path, bool create)
    {
        var candidate = await EmbeddedSilverHamuStore.OpenAsync(path, create);
        CloseStorage(); embedded = candidate;
        LocalizationBindings.Bind(connection, Label.TextProperty, "Embedded", Path.GetFileName(path));
        ToolTipProperties.SetText(connection, path);
        LocalizationBindings.Bind(status, Label.TextProperty, "EmbeddedConnection", path);
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log(L.Get(create ? "EmbeddedCreated" : "EmbeddedOpened", path));
        await RefreshTablesAsync();
    }

    private async Task BackupEmbeddedAsync()
    {
        var store = embedded ?? throw new InvalidOperationException(L.Get("OpenALocalDBToBackUp"));
        var suggested = Path.ChangeExtension(store.DatabasePath, ".backup");
        var path = await PickPathAsync(L.Get("BackUpCommittedLocalDB"), suggested, true);
        if (path is null) return;
        await store.BackupAsync(path);
        Log(L.Get("SavedBackup", path));
    }

    private async Task RestoreEmbeddedAsync()
    {
        if (inTransaction) throw new InvalidOperationException(L.Get("COMMITOrROLLBACKBeforeOpeningTheRestoredDB"));
        var directory = Path.GetDirectoryName(embedded?.DatabasePath) ?? FileSystem.AppDataDirectory;
        var source = await PickPathAsync(L.Get("ChooseABackupToRestore"), Path.Combine(directory, "silverhamu.backup"), false, browse: true);
        if (source is null) return;
        var suggested = Path.Combine(Path.GetDirectoryName(source)!, Path.GetFileNameWithoutExtension(source) + "-restored.ndb");
        var target = await PickPathAsync(L.Get("ChooseANewDestinationDB"), suggested, true);
        if (target is null) return;
        await EmbeddedSilverHamuStore.RestoreAsync(source, target);
        Log(L.Get("RestoredBackupToANewDB", target));
        await OpenEmbeddedPathAsync(target, false);
    }

    private void UpdateMode()
    {
        if (embedded is not null) inTransaction = embedded.InTransaction;
        if (folder is not null) inTransaction = folder.InTransaction;
        var edit = documents[activeDocument].Edit;
        UpdateEditHistoryControls();
        resultTable.IsEnabled = !busy;
        resultTable.IsEditingEnabled = edit is { IsCurrent: true, PreviewError: null } && !editScriptPending && edit.ConnectionVersion == connectionVersion && HasConnection && !IsReadRole;
        foreach (var button in guarded.Where(b => b.AutomationId is "Browse" or "ReloadEditResults"))
        {
            button.IsVisible = button.AutomationId == "Browse" ? edit is null : edit is not null;
            if (button.AutomationId == "ReloadEditResults") button.IsEnabled = !busy && edit is not null && HasConnection && edit.ConnectionVersion == connectionVersion;
        }
        previousPage.IsVisible = nextPage.IsVisible = lastPage.IsVisible = currentPage.IsVisible = pagePosition.IsVisible = loadedResult is not null;
        currentPage.IsEnabled = !busy && loadedResult is not null;
        previousPage.IsEnabled = !busy && loadedResult is not null && resultPage > 0;
        nextPage.IsEnabled = lastPage.IsEnabled = !busy && loadedResult is not null && resultPage + 1 < loadedResult.PageCount;
        foreach (var button in guarded.Where(b => b.AutomationId == "Execute"))
            button.IsEnabled = !busy && (edit is null || edit.IsCurrent && edit.PreviewError is null && edit.IncompleteRowCount == 0 && !editScriptPending && edit.ConnectionVersion == connectionVersion && HasConnection && (edit.PendingCount > 0 || resultTable.HasPendingEdit));
        backupButton.IsEnabled = !busy && embedded is not null;
        restoreButton.IsEnabled = !busy && !inTransaction;
        rawKvButton.IsEnabled = !busy && CanUseRawStore;
        snapshotButton.IsEnabled = !busy && embedded is not null && !IsReadRole;
        rawKvButton.IsVisible = snapshotButton.IsVisible = !IsReadRole;
        foreach (var button in guarded.Where(b => b.AutomationId is "Begin" or "Commit" or "Rollback" or "Checkpoint"))
        {
            button.IsVisible = edit is null && !IsReadRole && (button.AutomationId != "Checkpoint" || folder is null && HasConnection);
            button.IsEnabled = !busy && HasConnection && !IsReadRole &&
                (button.AutomationId is "Commit" or "Rollback" ? inTransaction : !inTransaction);
            ToolTipProperties.SetText(button, folder is not null
                ? L.Get("FolderTransactionHelp")
                : embedded is not null ? L.Get("ManageTransactionsInTheEmbeddedDB") : L.Get("ManageTransactionsInTheConnectedDB"));
        }
        LocalizationBindings.Bind(sqlHint, Label.TextProperty, "SqlEditorHint");
        if (folder is not null)
        {
            LocalizationBindings.Bind(sqlHint, Label.TextProperty, "FolderSqlHint");
        }
        if (IsReadRole)
        {
            LocalizationBindings.Bind(sqlHint, Label.TextProperty, "SHOWVIEWSSELECTPublishedViewsAreReadOnly");
        }
        if (edit is not null)
            LocalizationBindings.Bind(sqlHint, Label.TextProperty, edit.ConnectionVersion != connectionVersion ? "EditConnectionChanged" : !edit.IsCurrent ? "EditRefreshRequired" : "EditQueryHint");
    }

    private void CloseStorage()
    {
        connectionVersion++;
        foreach (var document in documents) { document.Loaded?.Dispose(); document.Loaded = null; }
        completionController?.Hide();
        folder?.Dispose(); folder = null; ClearLoadedResult();
        embedded?.Dispose(); embedded = null;
        client?.Dispose(); client = null;
    }

    private void AttachCompletion()
    {
        completionController?.Dispose(); completionController = null;
        if (historyEditor is not null) historyEditor.PreviewKeyDown -= OnEditHistoryKeyDown;
        historyEditor = null;
        if (editor.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.TextBox native)
        {
            native.AcceptsReturn = true;
            native.TextWrapping = Microsoft.UI.Xaml.TextWrapping.Wrap;
            historyEditor = native;
            native.PreviewKeyDown += OnEditHistoryKeyDown;
            completionController = new(native, () => folder is null ? tables : folder.Tables.Select(t => t.Schema));
        }
    }

    private Task DisconnectAsync()
    {
        var wasEmbedded = embedded is not null;
        CloseStorage(); inTransaction = false;
        LocalizationBindings.Bind(connection, Label.TextProperty, "NotConnected"); LocalizationBindings.Bind(status, Label.TextProperty, "Disconnected");
        ToolTipProperties.SetText(connection, L.Get("Disconnected"));
        LocalizationBindings.Bind(transaction, Label.TextProperty, "NoTransaction");
        result = new([], [], ""); tables.Clear(); RenderRows(); RenderExplorer();
        Log(wasEmbedded ? L.Get("ClosedTheEmbeddedDBDiscardedUncommittedChangesAnd") : L.Get("ClosedTheConnectionAndDiscardedUncommittedChanges"));
        return Task.CompletedTask;
    }

    private async Task ExecuteAsync()
    {
        if (documents[activeDocument].Edit is not null) { await ExecuteEditedBatchAsync(); return; }
        if (!HasConnection && folder is null) throw new InvalidOperationException(L.Get("ConnectToAServiceOrOpenADB"));
        var watch = Stopwatch.StartNew();
        ClearLoadedResult();
        result = new([], [], ""); RenderRows();
        IReadOnlyList<SqlResult> output;
        if (folder is not null)
        {
            var currentFolder = folder;
            var sql = editor.Text;
            if (SqlParser.Parse(sql) is [SelectRows query])
            {
                loadedResult = await Task.Run(() => currentFolder.LoadQueryAsync(query));
                watch.Stop();
                await ShowLoadedPageAsync(0);
                LocalizationBindings.Bind(timing, Label.TextProperty, "FetchAllNMs", watch.Elapsed.TotalMilliseconds);
                Log(L.Get("FetchedAllNRowsInNMsAnd", loadedResult.RowCount, watch.Elapsed.TotalMilliseconds));
                RenderExplorer();
                return;
            }
            output = await ExecuteFolderSqlAsync(sql);
        }
        else if (client?.SupportsSql == true) output = await ExecuteServiceSqlAsync(editor.Text);
        else output = await Database.ExecuteAsync(editor.Text);
        result = output[^1];
        filter.Text = "";
        RenderRows();
        timing.RemoveBinding(Label.TextProperty);
        timing.Text = $"{watch.Elapsed.TotalMilliseconds:N0} ms";
        foreach (var item in output) Log(item.Message);
        if (folder is not null)
        {
            ClearLoadedResult(); RenderExplorer();
            return;
        }
        try { await RefreshTablesAsync(); }
        catch (Exception error) { Log(L.Get("SQLCompletedButTheTableListCouldNot", error.Message)); }
    }

    private async Task RefreshTablesAsync()
    {
        if (folder is not null) { RenderExplorer(); return; }
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
        return response["results"]?.Deserialize<List<SqlResult>>() ?? throw new IOException(L.Get("CouldNotReadTheSQLResult"));
    }

    private async Task<JsonObject> SendAsync(JsonObject request)
    {
        var op = request["op"]!.ToString();
        try
        {
            var response = embedded is not null ? await embedded.RequestAsync(request) : await client!.RequestAsync(request);
            if (embedded is not null) inTransaction = embedded.InTransaction;
            else
            {
                if (op == "begin") inTransaction = true;
                if (op is "commit" or "rollback") inTransaction = false;
                if (op == "sql") inTransaction = client!.SqlTransactionActive;
            }
            return response;
        }
        catch (SilverHamuRequestException error)
        {
            if (embedded is not null) inTransaction = embedded.InTransaction;
            else
            {
                if (op == "commit") inTransaction = false;
                if (op == "sql") inTransaction = client!.SqlTransactionActive;
            }
            if (error.Message is "commit_uncertain" or "checkpoint_uncertain" or "reopen_required" or "unauthorized" or "sql_host_failed_outcome_unknown")
            { CloseStorage(); inTransaction = false; LocalizationBindings.Bind(connection, Label.TextProperty, "DisconnectedReconnectRequired"); tables.Clear(); RenderExplorer(); LocalizationBindings.Bind(status, Label.TextProperty, "ReconnectRequiredVerifyTheCommitOutcomeAgainstYour"); }
            throw;
        }
        catch
        {
            CloseStorage(); inTransaction = false;
            LocalizationBindings.Bind(connection, Label.TextProperty, "DisconnectedReconnectRequired"); tables.Clear(); RenderExplorer();
            LocalizationBindings.Bind(status, Label.TextProperty, "ConnectionInterruptedReopenTheDBOrReconnect");
            throw;
        }
        finally { transaction.Text = inTransaction ? L.Get("UncommittedTransaction") : L.Get("NoTransaction"); }
    }

    private async Task TransactionAsync(string op)
    {
        if (!HasConnection) throw new InvalidOperationException(L.Get("ConnectToAServiceOrOpenADB086"));
        if (folder is not null) await ExecuteFolderSqlAsync(op.ToUpperInvariant() + ";");
        else await SendAsync(new JsonObject { ["op"] = op });
        Log(L.Get("Completed", op.ToUpperInvariant()));
        await RefreshTablesAsync();
    }

    private async Task CheckpointAsync()
    {
        if (embedded is null && client is null) throw new InvalidOperationException(L.Get("ConnectToADBToRunACheckpoint"));
        if (inTransaction) throw new InvalidOperationException(L.Get("COMMITOrROLLBACKFirst"));
        var info = await SendAsync(new JsonObject { ["op"] = "checkpoint" });
        Log(L.Get("CheckpointCompletedGenerationRecords", info["version"], info["records"]));
    }

    private Task BrowseAsync()
    {
        if (folder is not null)
        {
            ClearLoadedResult(); filter.Text = "";
            result = folder.Summary(); RenderRows(); RenderExplorer();
            return Task.CompletedTask;
        }
        editor.Text = tables.Count > 0 ? $"SELECT * FROM {SqlNames.Quote(tables[0].Name)} FETCH FIRST 100 ROWS ONLY;" : IsReadRole ? "SHOW VIEWS;" : "SHOW TABLES;"; return ExecuteAsync();
    }

    private Task NewQueryAsync()
    {
        var query = tables.Count > 0 ? $"SELECT * FROM {SqlNames.Quote(tables[0].Name)} FETCH FIRST 100 ROWS ONLY;" : DefaultQuery;
        return AddQueryAsync(query);
    }

    private Task AddQueryAsync(string query)
    {
        CaptureDocument();
        documents.Add(new("", query) { Number = ++queryNumber });
        activeDocument = documents.Count - 1; RestoreDocument();
        return Task.CompletedTask;
    }

    private void RenderTabs()
    {
        foreach (var button in queryTabButtons) guarded.Remove(button);
        queryTabButtons.Clear();
        tabs.Clear();
        for (var i = 0; i < documents.Count; i++)
        {
            var index = i;
            var title = QueryTitle(documents[i]);
            var tab = Action(title, $"QueryTab{i}", () =>
            {
                CaptureDocument();
                activeDocument = index; RestoreDocument();
                return Task.CompletedTask;
            });
            tab.BackgroundColor = i == activeDocument ? CanvasColor : Surface;
            tab.TextColor = i == activeDocument ? Accent : Muted;
            tab.CornerRadius = 0;
            var close = Action("×", $"CloseQueryTab{i}", () => CloseQueryAsync(index));
            close.BackgroundColor = tab.BackgroundColor;
            close.TextColor = tab.TextColor;
            close.FontSize = 18;
            close.CornerRadius = 0;
            close.Padding = 0;
            close.MinimumWidthRequest = 32;
            SemanticProperties.SetDescription(close, L.Get("CloseQueryTab", title));
            ToolTipProperties.SetText(close, L.Get("CloseQueryTab", title));
            var group = new Grid { ColumnDefinitions = { new(GridLength.Auto), new(32) }, BackgroundColor = tab.BackgroundColor };
            group.Add(tab); group.Add(close, 1);
            queryTabButtons.Add(tab); queryTabButtons.Add(close);
            tabs.Add(group);
        }
        var addTab = Action("＋", "NewQuery", NewQueryAsync);
        addTab.MinimumWidthRequest = 42;
        addTab.BackgroundColor = Surface;
        SemanticProperties.SetDescription(addTab, L.Get("NewQuery"));
        ToolTipProperties.SetText(addTab, L.Get("NewQuery"));
        queryTabButtons.Add(addTab);
        tabs.Add(addTab);
    }

    private static string QueryTitle(QueryDocument document) => document.Edit is { } edit ? L.Get("EditTabTitle", edit.Table.Name, document.Number) : document.Title.Length == 0 ? L.Get("Query091", document.Number) : document.Title;

    private async Task CloseQueryAsync(int index)
    {
        CaptureDocument();
        var document = documents[index];
        if (document.Edit?.PendingCount > 0 || document.Text.ReplaceLineEndings("\n") != document.SavedText.ReplaceLineEndings("\n"))
        {
            bool discard;
            queryClosePromptOpen = true; UpdateBusyIndicator();
            try { discard = await DisplayAlertAsync(L.Get("DiscardQueryTitle"), L.Get(document.Edit?.PendingCount > 0 ? "EditCloseConfirm" : "DiscardQueryMessage", QueryTitle(document)), L.Get("DiscardQuery"), L.Get("Cancel")); }
            finally { queryClosePromptOpen = false; UpdateBusyIndicator(); }
            if (!discard) return;
        }
        document.Loaded?.Dispose();
        documents.RemoveAt(index);
        if (documents.Count == 0)
        {
            documents.Add(new("", DefaultQuery) { Number = ++queryNumber });
            activeDocument = 0;
        }
        else if (index < activeDocument) activeDocument--;
        else if (index == activeDocument) activeDocument = Math.Min(index, documents.Count - 1);
        RestoreDocument();
    }

    private async Task OpenAsync()
    {
        var path = await PickPathAsync(L.Get("OpenQuery"), Path.Combine(FileSystem.AppDataDirectory, "silverhamu-query.sql"), false);
        if (path is null) return;
        await using var stream = File.OpenRead(path);
        if (stream.CanSeek && stream.Length > 1024 * 1024) throw new InvalidOperationException(L.Get("TheQueryFileMustBeMiBOrSmaller"));
        using var reader = new StreamReader(stream);
        var text = await reader.ReadToEndAsync();
        await NewQueryAsync(); documents[activeDocument].Title = Path.GetFileName(path);
        documents[activeDocument].Text = documents[activeDocument].SavedText = text;
        editor.Text = text; RenderTabs();
    }

    private async Task SaveAsync()
    {
        var path = await PickPathAsync(L.Get("SaveQuery"), Path.Combine(FileSystem.AppDataDirectory, "silverhamu-query.sql"), true);
        if (path is null) return;
        await WriteNewFileAsync(path, editor.Text, new UTF8Encoding(false));
        documents[activeDocument].Text = documents[activeDocument].SavedText = editor.Text;
        documents[activeDocument].Title = Path.GetFileName(path); RenderTabs();
        Log(L.Get("SavedQuery", path));
    }

    private void BuildFileDialog()
    {
        var panel = new VerticalStackLayout { WidthRequest = 600, Padding = 28, Spacing = 16, BackgroundColor = Surface, HorizontalOptions = LayoutOptions.Center, VerticalOptions = LayoutOptions.Center };
        panel.Add(fileTitle);
        panel.Add(LocalizedText("FullFilePath", 13));
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
                    ?? throw new InvalidOperationException(L.Get("CouldNotOpenTheFilePickerEnterThe"));
                var selected = await DatabaseFilePicker.PickAsync(window, filePath.Text ?? "", fileTitle.Text);
                if (selected is not null) filePath.Text = selected;
            }
            catch (Exception error) { LocalizationBindings.Bind(fileError, Label.TextProperty, "CouldNotSelectAFile", error.Message); }
            finally { browseFile.IsEnabled = true; }
        };
        var buttons = new HorizontalStackLayout { Spacing = 8 };
        var submit = new Button { AutomationId = "FileSubmit", BackgroundColor = Accent, TextColor = CanvasColor }.Localized(Button.TextProperty, "OK").WithActionIcon();
        submit.Clicked += (_, _) =>
        {
            try
            {
                if (string.IsNullOrWhiteSpace(filePath.Text) || !Path.IsPathFullyQualified(filePath.Text))
                    throw new InvalidOperationException(L.Get("EnterTheFullFilePath"));
                var path = Path.GetFullPath(filePath.Text.Trim());
                if (savingFile && File.Exists(path)) throw new InvalidOperationException(L.Get("AFileWithThatNameAlreadyExistsChoose"));
                if (!savingFile && !File.Exists(path)) throw new InvalidOperationException(L.Get("FileNotFound"));
                if (!Directory.Exists(Path.GetDirectoryName(path))) throw new InvalidOperationException(L.Get("TheDestinationFolderWasNotFound"));
                fileOverlay.IsVisible = false; fileSelection?.TrySetResult(path);
            }
            catch (Exception error) { fileError.Text = error.Message; }
        };
        var cancel = new Button { AutomationId = "FileCancel" }.Localized(Button.TextProperty, "Cancel").WithActionIcon();
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
        fileHint.Text = saving ? L.Get("CreateTheDestinationFolderFirstExistingFilesWill")
            : browse ? L.Get("BrowseForAnExistingFileOrEnterThe") : L.Get("EnterTheFullPathOfAnExistingFile");
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
            explorer.Add(Text(L.Get("Tables", folder.Tables.Count), 13, true));
            foreach (var table in folder.Tables)
                explorer.Add(TableAction(folder.Label(table), $"FolderTable{explorer.Count}", () => { editor.Text = $"SELECT * FROM {SqlNames.Quote(table.Name)};"; return ExecuteAsync(); }, table.Schema));
            return;
        }
        explorer.Add(Text(embedded is not null ? L.Get("EmbeddedDatabase") : client is null ? L.Get("NoConnection") : L.Get("ServiceDatabase"), 13, true));
        if (!HasConnection) return;
        explorer.Add(LocalizedAction("RefreshTablesAndViews", "RefreshTables", async () => { await RefreshTablesAsync(); Log(L.Get("RefreshedTheListObjects", tables.Count)); }));
        explorer.Add(Text(L.Get("TablesAndViews", tables.Count), 11, color: Muted));
        foreach (var table in tables)
        {
            var caption = table.ViewQuery is null ? table.Name : $"VIEW  {table.Name}";
            explorer.Add(TableAction(caption, $"Table{SqlNames.Hex(table.Name)}", () => { editor.Text = $"SELECT * FROM {SqlNames.Quote(table.Name)} FETCH FIRST 100 ROWS ONLY;"; return ExecuteAsync(); }, table));
            foreach (var column in table.Columns)
                explorer.Add(Text($"    {column.Name}  {column.Type}{(column.PrimaryKey ? "  PK" : column.NotNull ? "  NOT NULL" : "")}", 11, color: Muted));
            foreach (var index in table.Indexes ?? [])
                explorer.Add(Text($"    INDEX  {index.Name} ({string.Join(", ", index.ColumnNames)})", 11, color: Muted));
        }
    }

    private void ClearLoadedResult()
    {
        if (ReferenceEquals(documents[activeDocument].Loaded, loadedResult)) documents[activeDocument].Loaded = null;
        loadedResult?.Dispose(); loadedResult = null; resultPage = 0;
    }

    private async Task ShowLoadedPageAsync(long page)
    {
        if (loadedResult is null || page < 0 || page >= loadedResult.PageCount) return;
        var loaded = loadedResult;
        var visible = await Task.Run(() => loaded.ReadPage(page));
        resultPage = page;
        filter.Text = ""; result = visible;
        var start = loaded.RowCount == 0 ? 0 : page * LoadedQueryResult.PageSize + 1;
        currentPage.Text = $"{page + 1:N0}";
        SemanticProperties.SetDescription(currentPage, L.Get("CurrentPageNOpenPageSelection", page + 1));
        LocalizationBindings.Bind(pagePosition, Label.TextProperty, "ResultPagePosition", loaded.PageCount, start, page * LoadedQueryResult.PageSize + result.Rows.Count, loaded.RowCount);
        RenderRows(); UpdateMode();
    }

    private async Task PickResultPageAsync()
    {
        if (busy || pagePickerOpen || loadedResult is null || currentPage.Handler?.PlatformView is not Microsoft.UI.Xaml.Controls.Button native) return;
        using var popup = new PageJumpPopup(native, loadedResult.PageCount, resultPage);
        long? page;
        pagePickerOpen = true; UpdateBusyIndicator();
        try { page = await popup.ShowAsync(); }
        finally { pagePickerOpen = false; UpdateBusyIndicator(); }
        if (page is { } selected) await RunActionAsync(() => ShowLoadedPageAsync(selected));
    }

    private Button TableAction(string caption, string id, Func<Task> action, SqlTable table)
    {
        var button = Action(caption, id, action).WithIcon(table.ViewQuery is null ? Microsoft.UI.Xaml.Controls.Symbol.List : Microsoft.UI.Xaml.Controls.Symbol.Preview);
        button.HorizontalOptions = LayoutOptions.Fill;
        void ConfigureMenu()
        {
            if (button.Handler?.PlatformView is Microsoft.UI.Xaml.Controls.Button native)
            {
                native.HorizontalContentAlignment = Microsoft.UI.Xaml.HorizontalAlignment.Left;
                var menu = new Microsoft.UI.Xaml.Controls.MenuFlyout();
                foreach (var template in ObjectQueryTemplates.Create(table))
                {
                    var item = new Microsoft.UI.Xaml.Controls.MenuFlyoutItem { Text = template.Caption, Icon = new Microsoft.UI.Xaml.Controls.SymbolIcon(ButtonIcons.QueryOperation(template.Operation)) };
                    Microsoft.UI.Xaml.Automation.AutomationProperties.SetAutomationId(item, $"ObjectQuery{template.Operation}");
                    item.Click += async (_, _) =>
                    {
                        if (busy) return;
                        if (!resultTable.TryCommitPendingEdit()) return;
                        completionController?.Hide();
                        if (template.EditResults) await RunActionAsync(() => OpenEditQueryAsync(table, template.Sql));
                        else await AddQueryAsync(template.Sql);
                        editor.Focus();
                    };
                    menu.Items.Add(item);
                }
                menu.Opening += (_, _) =>
                {
                    foreach (var item in menu.Items.OfType<Microsoft.UI.Xaml.Controls.MenuFlyoutItem>())
                        item.IsEnabled = !busy && (!(Microsoft.UI.Xaml.Automation.AutomationProperties.GetAutomationId(item) is "ObjectQuerySelectEdit" or "ObjectQuerySelectAllEdit") || HasConnection && !IsReadRole);
                };
                native.ContextFlyout = menu;
            }
        }
        button.HandlerChanged += (_, _) => ConfigureMenu();
        ConfigureMenu();
        return button;
    }

    private void RenderRows()
    {
        filterTimer.Stop();
        if (!resultTable.TryCommitPendingEdit()) return;
        var watch = Stopwatch.StartNew();
        var source = embedded is not null ? $"DB:{embedded.DatabasePath}" : connection.Text;
        var layoutKey = JsonSerializer.Serialize(new { Source = source, Columns = result.Columns });
        if (!savedColumnWidths.TryGetValue(layoutKey, out var widths))
            savedColumnWidths[layoutKey] = widths = Enumerable.Repeat(240d, result.Columns.Count).ToArray();
        var query = filter.Text ?? "";
        var visible = result.Rows.Select((row, index) => (Row: row, Number: (loadedResult is null ? 0 : resultPage * LoadedQueryResult.PageSize) + index + 1))
            .Where(item => query.Length == 0 || item.Row.Any(x => x.Display.Contains(query, StringComparison.OrdinalIgnoreCase))).ToArray();
        LocalizationBindings.Bind(count, Label.TextProperty, "Rows117", visible.Length, result.Rows.Count);
        var emptyText = result.Rows.Count == 0 ? L.Get("TheResultContainsNoRows") : L.Get("NoRowsMatchTheFilter");
        visibleEditRows = visible.Select(item => checked((int)(item.Number - 1))).ToArray();
        resultTable.SetData(result.Columns, visible.Select(item => item.Row.Select((x, column) => documents[activeDocument].Edit is { } session ? session.DisplayValue(checked((int)item.Number - 1), column) : x.Display).ToArray()).ToArray(), widths, emptyText, visible.Select(item => item.Number).ToArray(),
            documents[activeDocument].Edit is { } editSession
                ? visible.SelectMany((item, row) => Enumerable.Range(0, result.Columns.Count).Where(column => editSession.IsModified(checked((int)item.Number - 1), column)).Select(column => new ResultAddress(row, column))).ToHashSet()
                : null,
            visible.Select(item => documents[activeDocument].Edit is { } session ? session.IsDeleted(checked((int)item.Number - 1)) ? ResultRowState.Deleted
                : session.IsAdded(checked((int)item.Number - 1)) ? ResultRowState.Added : ResultRowState.Original : ResultRowState.Original).ToArray(),
            documents[activeDocument].Edit is { } rowSession ? visible.Select(item => rowSession.RowLabel(checked((int)item.Number - 1))).ToArray() : null);
        Debug.WriteLine($"RenderRows: rows={visible.Length}, prepare/apply={watch.Elapsed.TotalMilliseconds:F1}ms");
    }
    private Button Action(string caption, string id, Func<Task> action, bool primary = false)
    {
        var button = new Button { Text = caption, AutomationId = id, FontSize = 12, CornerRadius = 5, Padding = new Thickness(12, 6), BackgroundColor = primary ? Accent : Surface, TextColor = primary ? CanvasColor : Colors.White, MinimumHeightRequest = 34 }.WithActionIcon();
        SemanticProperties.SetDescription(button, caption);
        guarded.Add(button);
        button.Clicked += async (_, _) => await RunActionAsync(action);
        return button;
    }

    private async Task RunActionAsync(Func<Task> action)
    {
        if (busy) return;
        if (!resultTable.TryCommitPendingEdit()) return;
        SynchronizeEditScript(render: false);
        busy = true;
        completionController?.Hide();
        UpdateBusyIndicator();
        foreach (var item in guarded) item.IsEnabled = false;
        try { await action(); }
        catch (Exception error)
        {
            Log(L.Get("Error", error.Message));
            if (overlay.IsVisible) connectionError.Text = error.Message;
            LocalizationBindings.Bind(timing, Label.TextProperty, "ErrorCheckMessages");
        }
        finally
        {
            busy = false; UpdateBusyIndicator(); foreach (var item in guarded) item.IsEnabled = true; UpdateMode();
        }
    }

    private void UpdateBusyIndicator()
    {
        var visible = busy && !fileOverlay.IsVisible && !csvOverlay.IsVisible && !settingsOverlay.IsVisible && !pagePickerOpen && !queryClosePromptOpen;
        busyIndicator.IsVisible = visible;
        busyAnimation.SetPlaying(visible && new global::Windows.UI.ViewManagement.UISettings().AnimationsEnabled);
    }

    private void Log(string message)
    {
        var text = $"{DateTime.Now:HH:mm:ss}  {message}\n{messages.Text}";
        messages.Text = text[..Math.Min(12000, text.Length)];
    }
    private static Label Text(string value, double size, bool bold = false, Color? color = null) => new() { Text = value, FontSize = size, TextColor = color ?? Colors.White, FontAttributes = bold ? FontAttributes.Bold : FontAttributes.None, VerticalTextAlignment = TextAlignment.Center };
    private static Entry Input(string id, string value) => new() { AutomationId = id, Text = value, FontSize = 13, TextColor = Colors.White, BackgroundColor = CanvasColor, MinimumHeightRequest = 36 };
    private sealed class QueryDocument(string title, string text)
    {
        public string Title = title, Text = text, SavedText = text, Filter = "";
        public int Number = 1;
        public SqlResult Result = new([], [], "");
        public LoadedQueryResult? Loaded;
        public long Page;
        public TableEditSession? Edit;
    }
}
