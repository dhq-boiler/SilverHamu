using SilverHamu.Owner.Controls;
using SilverHamu.Owner.Services;
using SilverHamu.Sql;

namespace SilverHamu.Owner;

public sealed partial class MainPage
{
    private int[] visibleEditRows = [];

    private void CaptureDocument()
    {
        SynchronizeEditScript(render: false);
        var document = documents[activeDocument];
        document.Text = editor.Text; document.Result = result; document.Filter = filter.Text ?? "";
        document.Loaded = loadedResult; document.Page = resultPage;
    }

    private void RestoreDocument()
    {
        var document = documents[activeDocument];
        editScriptTimer.Stop(); editScriptPending = false;
        SetEditText(document.Text); result = document.Edit?.Result ?? document.Result;
        loadedResult = document.Loaded; resultPage = document.Page; filter.Text = document.Filter;
        RefreshLanguage();
    }

    private void ConfigureQueryLayout()
    {
        if (workspacePane is null) return;
        bool editing = documents[activeDocument].Edit is not null;
        Grid.SetRow(resultHeaderPanel, editing ? 1 : 4);
        Grid.SetRow(resultTable, editing ? 2 : 5);
        Grid.SetRow(queryCommands, editing ? 3 : 1);
        Grid.SetRow(editorPanel, editing ? 4 : 2);
        Grid.SetRow(sqlHint, editing ? 5 : 3);
        GridLength[] heights = editing
            ? [42, 44, new(1, GridUnitType.Star), 48, new(1, GridUnitType.Star), 32, 64]
            : [42, 48, new(0.9, GridUnitType.Star), 32, 92, new(1.1, GridUnitType.Star), 88];
        for (int i = 0; i < heights.Length; i++) workspacePane.RowDefinitions[i].Height = heights[i];
        editor.Placeholder = editing ? L.Get("EditSqlPlaceholder") : "";
    }

    private Task<IReadOnlyList<SqlResult>> ExecuteOwnerSqlAsync(string sql) =>
        folder is not null ? ExecuteFolderSqlAsync(sql) : client?.SupportsSql == true ? ExecuteServiceSqlAsync(sql) : Database.ExecuteAsync(sql);

    private async Task<IReadOnlyList<SqlResult>> ExecuteFolderSqlAsync(string sql)
    {
        var current = folder ?? throw new InvalidOperationException();
        try { return await Task.Run(() => current.ExecuteAsync(sql)); }
        catch (SilverHamuRequestException error) when (error.Message is "commit_uncertain" or "reopen_required")
        {
            CloseStorage(); tables.Clear(); RenderExplorer();
            LocalizationBindings.Bind(connection, Label.TextProperty, "DisconnectedReconnectRequired");
            LocalizationBindings.Bind(status, Label.TextProperty, "ReconnectRequiredVerifyTheCommitOutcomeAgainstYour");
            throw;
        }
        finally
        {
            if (SqlParser.Parse(sql).Any(statement => statement is InsertStatement or UpdateRows or DeleteRows
                || statement is TransactionStatement { Operation: "rollback" or "rollback_to" or "commit" }))
                foreach (var document in documents) if (document.Edit is { } edit) edit.IsCurrent = false;
            inTransaction = folder?.InTransaction ?? false;
            transaction.Text = inTransaction ? L.Get("UncommittedTransaction") : L.Get("NoTransaction");
        }
    }

    private async Task<TableEditSession> LoadEditSessionAsync(SqlTable table, string selectSql)
    {
        var schema = (folder is not null ? folder.Tables.Select(t => t.Schema) : await Database.GetTablesAsync()).SingleOrDefault(t => string.Equals(t.Name, table.Name, StringComparison.OrdinalIgnoreCase))
            ?? throw new InvalidOperationException(L.Get("EditSchemaMismatch"));
        var selected = folder is not null ? await Task.Run(() => folder.ReadEditQueryAsync(selectSql)) : (await ExecuteOwnerSqlAsync(selectSql)).Single();
        return new(schema, selected, selectSql, connectionVersion);
    }

    private async Task OpenEditQueryAsync(SqlTable table, string selectSql)
    {
        if (!HasConnection || IsReadRole || table.ViewQuery is not null)
            throw new InvalidOperationException(L.Get("EditWritableTableRequired"));
        var session = await LoadEditSessionAsync(table, selectSql);
        CaptureDocument();
        documents.Add(new("", "") { Number = ++queryNumber, Edit = session, Result = session.Result });
        activeDocument = documents.Count - 1;
        RestoreDocument();
        Log(L.Get("EditLoaded", session.Table.Name, session.Result.Rows.Count));
    }

    private TableEditSession RequireEditSession()
    {
        var session = documents[activeDocument].Edit ?? throw new InvalidOperationException(L.Get("EditWritableTableRequired"));
        if (!HasConnection || session.ConnectionVersion != connectionVersion || IsReadRole)
            throw new InvalidOperationException(L.Get("EditConnectionChanged"));
        return session;
    }

    private async Task ExecuteEditedBatchAsync()
    {
        var session = RequireEditSession();
        if (!session.IsCurrent) throw new InvalidOperationException(L.Get("EditRefreshRequired"));
        SynchronizeEditScript(render: false);
        if (session.PreviewError is { } previewError) throw new InvalidOperationException(L.Get("EditPreviewInvalid", previewError));
        if (session.IncompleteRowCount > 0) throw new InvalidOperationException(L.Get("EditIncompleteRows", session.IncompleteRowCount));
        if (session.PendingCount == 0) return;
        bool committed = await TableEditBatch.ExecuteAsync(editor.Text, session.Table.Name, ExecuteOwnerSqlAsync, () => folder?.InTransaction ?? embedded?.InTransaction ?? inTransaction);
        session.MarkApplied();
        var document = documents[activeDocument];
        SetEditText(""); document.SavedText = "";
        Log(L.Get(committed ? "EditApplied" : "EditAppliedInTransaction"));
        try
        {
            document.Edit = await LoadEditSessionAsync(session.Table, session.SelectSql);
            result = document.Result = document.Edit.Result;
        }
        catch (Exception error) { Log(L.Get("EditReloadFailed", error.Message)); }
        RenderRows(); UpdateMode();
    }

    private async Task ReloadEditAsync()
    {
        var session = RequireEditSession();
        if (session.PendingCount > 0 || !string.IsNullOrWhiteSpace(editor.Text))
        {
            bool discard;
            queryClosePromptOpen = true; UpdateBusyIndicator();
            try { discard = await DisplayAlertAsync(L.Get("EditReloadTitle"), L.Get("EditReloadConfirm"), L.Get("EditDiscardReload"), L.Get("Cancel")); }
            finally { queryClosePromptOpen = false; UpdateBusyIndicator(); }
            if (!discard) return;
        }
        var refreshed = await LoadEditSessionAsync(session.Table, session.SelectSql);
        var document = documents[activeDocument];
        document.Edit = refreshed; result = document.Result = refreshed.Result;
        SetEditText(""); document.SavedText = "";
        filter.Text = ""; RenderRows(); UpdateMode();
    }

    private ResultEditValue ReadResultEditValue(int visibleRow, int columnIndex)
    {
        var session = RequireEditSession();
        var value = session.Result.Rows[visibleEditRows[visibleRow]][columnIndex];
        bool nullable = session.CanBeNull(columnIndex);
        return new(value.Kind == "BLOB" ? "0x" + value.Text : value.Text ?? "", value.IsNull && nullable, nullable);
    }

    private Task AddEditRowAsync()
    {
        var session = RequireEditSession();
        SetEditText(session.AddRow()); result = session.Result;
        filter.Text = ""; RenderRows(); UpdateMode();
        int row = result.Rows.Count - 1;
        int column = Enumerable.Range(0, session.Table.Columns.Count).FirstOrDefault(c => !session.CanBeNull(c) && result.Rows[row][c].IsNull);
        Dispatcher.Dispatch(() => resultTable.BeginCellEdit(row, column));
        return Task.CompletedTask;
    }

    private Task DeleteEditRowsAsync(int[] rows)
    {
        var session = RequireEditSession();
        SetEditText(session.DeleteRows(rows)); result = session.Result;
        RenderRows(); UpdateMode();
        return Task.CompletedTask;
    }

    private Task DuplicateEditRowsAsync(int[] rows)
    {
        var session = RequireEditSession();
        int first = session.Result.Rows.Count;
        SetEditText(session.DuplicateRows(rows)); result = session.Result;
        if (result.Rows.Count == first) return Task.CompletedTask;
        filter.Text = ""; RenderRows(); UpdateMode();
        int column = Enumerable.Range(0, session.Table.Columns.Count).FirstOrDefault(session.IsUniqueColumn);
        Dispatcher.Dispatch(() => resultTable.BeginCellEdit(first, column));
        return Task.CompletedTask;
    }

    private void OnResultEditRequested(object? sender, ResultEditRequest request)
    {
        try
        {
            if (busy || !resultTable.IsEditingEnabled) throw new InvalidOperationException(L.Get("EditRefreshRequired"));
            var session = RequireEditSession();
            int row = visibleEditRows[request.Row];
            if (session.IsDeleted(row)) throw new InvalidOperationException(L.Get("EditDeletedRow"));
            var previous = session.Result.Rows[row][request.Column];
            var value = !request.IsNull && previous.Kind == "TEXT" && previous.Text!.ReplaceLineEndings("\n") == request.Text.ReplaceLineEndings("\n")
                ? previous : session.ParseValue(request.Column, request.Text, request.IsNull);
            if (session.IsAdded(row)) SetEditText(session.ChangeInsertedCell(row, request.Column, value));
            else
            {
                var sql = session.Stage(row, request.Column, value);
                if (sql.Length > 0)
                {
                    var existing = editor.Text?.TrimEnd() ?? "";
                    SetEditText(session.Synchronize(existing + (existing.Length == 0 ? "" : Environment.NewLine) + sql));
                }
            }
            result = session.Result;
            request.Accepted = true;
        }
        catch (Exception error) { request.Error = error.Message; }
    }
}
