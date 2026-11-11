namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    public static async Task<SqlResult> ExecuteExternalWriteAsync(SqlTable table, SqlStatement statement,
        IAsyncEnumerable<SqlRow> source, Func<SqlRow?, SqlRow?, Task> stage)
    {
        string? target = statement switch { InsertStatement i => i.Table, UpdateRows u => u.Table, DeleteRows d => d.Table, _ => null };
        if (!string.Equals(target, table.Name, StringComparison.OrdinalIgnoreCase) || table.ViewQuery is not null)
            throw new SqlException("書庫の更新には対象テーブルのINSERT・UPDATE・DELETEを指定してください。");
        if (table.Columns.Any(c => c.Unique || c.PrimaryKey || c.Reference is not null || c.Checks is { Count: > 0 })
            || table.CompositePrimaryKey is { Count: > 0 }
            || table.Checks is { Count: > 0 })
            throw new SqlException("制約・トリガーを持つテーブルは通常のSQL DB接続で更新してください。");
        BindPreviewTypes(table, statement);
        var returned = new List<SqlRow>();
        long affected = 0;
        void Return(SqlRow row)
        {
            affected++;
            if (statement.Returning is null) return;
            if (returned.Count >= MaxRows) throw new SqlException($"RETURNINGは{MaxRows}行までです。対象を絞ってください。");
            returned.Add(row);
        }
        switch (statement)
        {
            case InsertStatement insert:
                if (insert.Conflict is not null) throw new SqlException("書庫のINSERTはON CONFLICTに対応していません。");
                var columns = InsertColumns(table, insert.Columns);
                foreach (var values in InsertValuesForColumns(insert, columns))
                {
                    var row = MakeInsertedRow(table, columns, values);
                    await stage(null, row); Return(row);
                }
                break;
            case UpdateRows update:
                ValidatePredicate(update.Where, table.Columns);
                var assignments = ResolveAssignments(table, update.Assignments);
                await foreach (var row in source)
                {
                    if (!Matches(update.Where, row.Values)) continue;
                    var values = new Dictionary<string, SqlValue>(row.Values, StringComparer.OrdinalIgnoreCase);
                    ApplyAssignments(table, values, assignments, row.Values);
                    var next = new SqlRow(row.StorageKey, values);
                    await stage(row, next); Return(next);
                }
                break;
            case DeleteRows delete:
                ValidatePredicate(delete.Where, table.Columns);
                await foreach (var row in source)
                {
                    if (!Matches(delete.Where, row.Values)) continue;
                    await stage(row, null); Return(row);
                }
                break;
        }
        string action = statement is InsertStatement ? "挿入" : statement is UpdateRows ? "更新" : "削除";
        var result = Empty($"{affected:N0} 行を{action}しました。");
        return statement.Returning is { } projections ? ReturnRows(table, projections, returned, result) : result;
    }
}
