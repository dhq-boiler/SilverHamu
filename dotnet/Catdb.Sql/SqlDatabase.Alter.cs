using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private async Task<SqlResult> AlterTableAsync(SqlTable table, SqlStatement statement, List<SqlRow> rows,
        List<(string Key, byte[]? Value)> changes)
    {
        SqlTable updated;
        SqlColumn column;
        string? oldName = null;
        if (statement is AddColumn add)
        {
            column = add.Column;
            if (table.Columns.Count >= 64) throw new SqlException("列数は64以下にしてください。");
            if (column.PrimaryKey && (table.CompositePrimaryKey is not null || table.Columns.Any(c => c.PrimaryKey))) throw new SqlException("主キーは既に存在します。");
            if (column.Default is not null) Coerce(column, column.Default);
            updated = table with { Columns = [.. table.Columns, column], PrimaryIndex = table.PrimaryIndex || column.PrimaryKey };
        }
        else
        {
            var rename = (RenameColumn)statement;
            var original = Column(table, rename.Column);
            oldName = original.Name;
            await RestrictReferencedSchemaAsync(table, oldName);
            column = original with { Name = rename.NewName };
            updated = table with
            {
                Columns = table.Columns.Select(c => c == original ? column : c).ToList(),
                CompositePrimaryKey = table.CompositePrimaryKey?.Select(c => c == oldName ? column.Name : c).ToList(),
                Indexes = table.Indexes?.Select(i => i with
                {
                    Column = i.Column == oldName ? column.Name : i.Column,
                    Columns = i.Columns?.Select(c => c == oldName ? column.Name : c).ToList()
                }).ToList()
            };
        }
        if (table.Columns.Any(c => c.Name != oldName && c.Name.Equals(column.Name, StringComparison.OrdinalIgnoreCase)))
            throw new SqlException($"列 '{column.Name}' は既に存在します。");

        foreach (var view in await GetViewsAsync())
        {
            if (!ViewUsesTable(view, table.Name)) continue;
            var query = (SelectRows)SqlParser.Parse(view.ViewQuery!).Single();
            var description = await DescribeViewAsync(new(view.Name, query, view.ViewQuery!, false), updated);
            if (!view.Columns.SequenceEqual(description.Columns))
                throw new SqlException($"ビュー '{view.Name}' の公開列が変わります。先にビューを再定義してください。");
        }
        long budget = 0;
        foreach (var row in rows)
        {
            var value = column.Default ?? SqlValue.Null;
            if (oldName is not null) { value = row.Values[oldName]; row.Values.Remove(oldName); }
            row.Values[column.Name] = value;
            ValidateRow(updated, row.Values);
            var bytes = Serialize(row.Values);
            budget += bytes.Length + Encoding.UTF8.GetByteCount(row.StorageKey);
            if (budget > MaxReadBytes) throw new SqlException("列変更の書き換えが8 MiBを超えています。");
            changes.Add((row.StorageKey, bytes));
            if (oldName is null && column.PrimaryKey)
                changes.Add((IndexKey(updated, value), Encoding.UTF8.GetBytes(row.StorageKey)));
        }
        ValidateUnique(updated, rows);
        ValidateChecks(updated, rows);
        await ValidateOutgoingReferencesAsync(updated, rows);
        changes.Add((TableKey(table.Name), Serialize(updated)));
        return Empty($"テーブル '{table.Name}' の列定義を変更しました。");
    }
}
