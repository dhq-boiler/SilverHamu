using System.Text;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed partial class TableEditSession
{
    private abstract record GeneratedOperation(int Row);
    private sealed record GeneratedEdit(int Row, int Column, SqlValue Value) : GeneratedOperation(Row);
    private sealed record GeneratedInsert(int Row, SqlValue[] Values) : GeneratedOperation(Row);
    private sealed record GeneratedDelete(int Row) : GeneratedOperation(Row);
    private SqlValue[][] originalRows;
    private readonly Dictionary<string, GeneratedOperation> generated = new(StringComparer.Ordinal);
    private readonly List<string> history = [""];
    private int historyIndex;
    public string Script { get; private set; } = "";
    public string? PreviewError { get; private set; }
    public bool CanUndo => IsCurrent && historyIndex > 0;
    public bool CanRedo => IsCurrent && historyIndex + 1 < history.Count;
    public bool IsModified(int row, int column) => !IsDeleted(row) && (IsAdded(row) || Result.Rows[row][column] != originalRows[rowIdentities[row]][column]);
    public int ModifiedCellCount { get; private set; }

    private static string StatementKey(string text) => PartKey(SqlParser.ParseWithSource(text).Single(), text);
    private static string PartKey(SqlParser.ParsedStatement part, string source)
    {
        string marker = part.Statement is InsertStatement ? System.Text.RegularExpressions.Regex.Match(source.Substring(part.Start, part.Length), @"^INSERT\s*/\*\s*\+([0-9]+)\s*\*/", System.Text.RegularExpressions.RegexOptions.IgnoreCase).Value : "";
        return part.Key + marker;
    }

    public string Synchronize(string sql, bool recordHistory = true)
    {
        if (!IsCurrent) throw new InvalidOperationException(L.Get("EditRefreshRequired"));
        sql = sql.ReplaceLineEndings("\n");
        var rows = originalRows.Select(row => (SqlValue[])row.Clone()).ToList();
        var identities = Enumerable.Range(0, rows.Count).ToList();
        var deleted = new HashSet<int>();
        try
        {
            var parts = SqlParser.ParseWithSource(sql);
            var rewritten = new StringBuilder(sql);
            var replacements = new List<(int Start, int Length, string Text)>();
            foreach (var part in parts)
            {
                if (!TableEditBatch.IsAllowed(part.Statement, Table.Name)) throw new InvalidOperationException(L.Get("EditUpdatesOnly"));
                string key = PartKey(part, sql);
                if (generated.TryGetValue(key, out var operation))
                {
                    if (operation is GeneratedInsert insert)
                    {
                        if (identities.Contains(insert.Row)) throw new InvalidOperationException(L.Get("EditAffectedRowMismatch"));
                        rows.Add((SqlValue[])insert.Values.Clone()); identities.Add(insert.Row);
                        continue;
                    }
                    int index = identities.IndexOf(operation.Row);
                    if (index < 0 || deleted.Contains(operation.Row)) throw new InvalidOperationException(L.Get("EditDeletedRow"));
                    string rebased;
                    if (operation is GeneratedEdit edit)
                    {
                        rebased = BuildUpdate(rows[index], edit.Column, edit.Value);
                        rows[index][edit.Column] = edit.Value;
                    }
                    else { rebased = BuildDelete(rows[index]); deleted.Add(operation.Row); }
                    generated[StatementKey(rebased)] = operation;
                    if (key != StatementKey(rebased)) replacements.Add((part.Start, part.Length, rebased.TrimEnd(';')));
                }
                else
                {
                    var active = rows.Where((_, index) => !deleted.Contains(identities[index])).ToArray();
                    switch (part.Statement)
                    {
                        case UpdateRows update: SqlDatabase.PreviewUpdate(Table, active, update); break;
                        case DeleteRows delete:
                            var removed = SqlDatabase.PreviewDelete(Table, active, delete);
                            deleted.Add(identities[rows.IndexOf(removed)]);
                            break;
                        case InsertStatement insert:
                            var values = SqlDatabase.PreviewInsert(Table, insert);
                            int identity = nextInsertedIdentity--;
                            rows.Add(values); identities.Add(identity);
                            string replacement = BuildInsert(identity, values);
                            generated[StatementKey(replacement)] = new GeneratedInsert(identity, values);
                            replacements.Add((part.Start, part.Length, replacement.TrimEnd(';')));
                            break;
                    }
                }
            }
            foreach (var replacement in replacements.AsEnumerable().Reverse())
                rewritten.Remove(replacement.Start, replacement.Length).Insert(replacement.Start, replacement.Text);
            sql = rewritten.ToString();
            Result = Result with { Rows = rows.ToArray() };
            rowIdentities = identities; deletedIdentities = deleted;
            IncompleteRowCount = identities.Where((id, row) => id < 0 && !deleted.Contains(id) && rows[row].Where((value, column) => !CanBeNull(column) && value.IsNull).Any()).Count();
            ModifiedCellCount = rows.Select((row, index) => identities[index] < 0 || deleted.Contains(identities[index]) ? 0
                : row.Where((value, column) => value != originalRows[identities[index]][column]).Count()).Sum();
            PendingCount = parts.Count;
            PreviewError = null;
        }
        catch (Exception error) { PreviewError = error.Message; }
        Script = sql;
        if (recordHistory && history[historyIndex] != sql)
        {
            history.RemoveRange(historyIndex + 1, history.Count - historyIndex - 1);
            history.Add(sql); historyIndex++;
        }
        return Script;
    }

    public string Undo() => CanUndo ? Synchronize(history[--historyIndex], recordHistory: false) : Script;
    public string Redo() => CanRedo ? Synchronize(history[++historyIndex], recordHistory: false) : Script;

    private void ClearHistory()
    {
        history.Clear(); history.Add(""); historyIndex = 0; Script = ""; PreviewError = null;
        generated.Clear();
        ModifiedCellCount = 0;
        Result = Result with { Rows = Result.Rows.Where((_, row) => !IsDeleted(row)).Select(row => (SqlValue[])row.Clone()).ToArray() };
        originalRows = Result.Rows.Select(row => (SqlValue[])row.Clone()).ToArray();
        rowIdentities = Enumerable.Range(0, originalRows.Length).ToList(); deletedIdentities.Clear();
        IncompleteRowCount = 0; nextInsertedIdentity = -1;
    }
}
