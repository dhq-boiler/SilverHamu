using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private async Task<SqlResult> AlterTableAsync(SqlTable table, SqlStatement statement, List<SqlRow> rows,
        List<(string Key, byte[]? Value)> changes)
    {
        SqlTable updated;
        Action<SqlRow>? transform = null;
        List<SqlRow>? beforeTypeChange = null;
        switch (statement)
        {
            case AddColumn add:
                var added = await ResolveColumnTypeAsync(add.Column);
                if (table.Columns.Count >= 64) throw new SqlException("列数は64以下にしてください。");
                if (table.Columns.Any(c => SameTable(c.Name, added.Name))) throw new SqlException($"列 '{added.Name}' は既に存在します。");
                if (added.PrimaryKey && (table.CompositePrimaryKey is not null || table.Columns.Any(c => c.PrimaryKey)))
                    throw new SqlException("主キーは既に存在します。");
                updated = table with { Columns = [.. table.Columns, added], PrimaryIndex = table.PrimaryIndex || added.PrimaryKey };
                transform = row =>
                {
                    var value = added.Default ?? SqlValue.Null;
                    row.Values.Add(added.Name, value);
                    if (added.PrimaryKey && !value.IsNull)
                        changes.Add((IndexKey(updated, value), Encoding.UTF8.GetBytes(row.StorageKey)));
                };
                break;
            case RenameColumn rename:
                var original = Column(table, rename.Column);
                var oldName = original.Name;
                var newName = rename.NewName;
                await RestrictReferencedSchemaAsync(table, oldName);
                if (table.Columns.Any(c => c != original && SameTable(c.Name, newName))) throw new SqlException($"列 '{newName}' は既に存在します。");
                string Renamed(string c) => SameTable(c, oldName) ? newName : c;
                updated = table with
                {
                    Columns = table.Columns.Select(c => c == original ? c with { Name = newName } : c).ToList(),
                    CompositePrimaryKey = table.CompositePrimaryKey?.Select(Renamed).ToList(),
                    Indexes = table.Indexes?.Select(i => i with { Column = Renamed(i.Column), Columns = i.Columns?.Select(Renamed).ToList() }).ToList(),
                    ForeignKeys = table.ForeignKeys?.Select(fk => fk with { Columns = fk.Columns.Select(Renamed).ToList() }).ToList()
                };
                transform = row =>
                {
                    var value = row.Values[oldName];
                    row.Values.Remove(oldName);
                    row.Values.Add(newName, value);
                };
                break;
            case DropColumn drop:
                var removed = Column(table, drop.Column);
                if (table.Columns.Count == 1) throw new SqlException("テーブルには1列以上必要です。");
                await RestrictReferencedSchemaAsync(table, removed.Name);
                bool UsesColumn(IEnumerable<string> columns) => columns.Contains(removed.Name, StringComparer.OrdinalIgnoreCase);
                if (table.CompositePrimaryKey is { } pk && UsesColumn(pk) ||
                    ForeignKeys(table).Any(fk => fk.Columns.Count > 1 && UsesColumn(fk.Columns)))
                    throw new SqlException("複合キーの一部の列を削除できません。");
                foreach (var index in (table.Indexes ?? []).Where(i => UsesColumn(i.ColumnNames)))
                    foreach (var entry in await ScanAsync(SecondaryPrefix(table, index), MaxRows)) changes.Add((entry.Key, null));
                if (removed.PrimaryKey && table.PrimaryIndex)
                    foreach (var entry in await ScanAsync(IndexPrefix(table.Name), MaxRows)) changes.Add((entry.Key, null));
                updated = table with
                {
                    Columns = table.Columns.Where(c => c != removed).ToList(),
                    PrimaryIndex = table.PrimaryIndex && !removed.PrimaryKey,
                    Indexes = table.Indexes?.Where(i => !UsesColumn(i.ColumnNames)).ToList(),
                    ForeignKeys = table.ForeignKeys?.Where(fk => !UsesColumn(fk.Columns)).ToList()
                };
                transform = row => row.Values.Remove(removed.Name);
                break;
            case AlterColumn alter:
                var column = Column(table, alter.Column);
                if (alter.Operation == SqlColumnAlteration.DropNotNull && (column.PrimaryKey ||
                    table.CompositePrimaryKey?.Contains(column.Name, StringComparer.OrdinalIgnoreCase) == true))
                    throw new SqlException("主キーのNOT NULLを削除できません。");
                var replacement = alter.Operation switch
                {
                    SqlColumnAlteration.Type => column with { Type = alter.Type!, MaxLength = alter.MaxLength, DistinctType = null },
                    SqlColumnAlteration.SetDefault => column with { Default = alter.Default },
                    SqlColumnAlteration.DropDefault => column with { Default = null },
                    SqlColumnAlteration.SetNotNull => column with { NotNull = true },
                    SqlColumnAlteration.DropNotNull => column with { NotNull = false },
                    _ => throw new SqlException("未対応の列変更です。")
                };
                replacement = await ResolveColumnTypeAsync(replacement);
                updated = table with { Columns = table.Columns.Select(c => c == column ? replacement : c).ToList() };
                if (ComparisonType(column) != ComparisonType(replacement))
                    await RestrictReferencedSchemaAsync(table, column.Name);
                if (alter.Operation == SqlColumnAlteration.Type)
                {
                    beforeTypeChange = rows;
                    rows = rows.Select(row =>
                    {
                        var value = Coerce(replacement, row.Values[column.Name]);
                        if (value == row.Values[column.Name]) return row;
                        var values = new Dictionary<string, SqlValue>(row.Values, StringComparer.OrdinalIgnoreCase)
                            { [column.Name] = value };
                        return new SqlRow(row.StorageKey, values);
                    }).ToList();
                }
                break;
            default:
                throw new SqlException("未対応の列変更です。");
        }
        foreach (var view in await GetViewsAsync())
        {
            if (!ViewUsesTable(view, table.Name)) continue;
            var query = (SelectRows)SqlParser.Parse(view.ViewQuery!).Single();
            var definition = new CreateView(view.Name, query, view.ViewQuery!, false);
            var description = await DescribeViewAsync(definition, updated);
            if (!view.Columns.SequenceEqual(description.Columns))
            {
                // Earlier catalogs reported numeric literals as REAL. Compare both schemas
                // with today's binder before attributing a type change to this ALTER.
                var before = await DescribeViewAsync(definition, table);
                if (!before.Columns.SequenceEqual(description.Columns))
                    throw new SqlException($"ビュー '{view.Name}' の公開列が変わります。先にビューを再定義してください。");
            }
        }
        foreach (var row in rows)
        {
            transform?.Invoke(row);
            ValidateRow(updated, row.Values);
            if (transform is not null) changes.Add((row.StorageKey, Serialize(row.Values)));
        }
        ValidateUnique(updated, rows);
        await ValidateChecksAsync(updated, rows);
        await ValidateOutgoingReferencesAsync(updated, rows);
        if (beforeTypeChange is not null)
            RecordRowChanges(new RowSet(updated, beforeTypeChange, rows), changes);
        changes.Add((TableKey(table.Name), Serialize(updated)));
        return Empty($"テーブル '{table.Name}' の列定義を変更しました。");
    }
}
