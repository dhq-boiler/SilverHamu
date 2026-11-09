namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    // Use the same name binding, projection, grouping, and joins for externally stored rows.
    public static async Task<SqlResult> QueryReadOnlyAsync(SelectRows query, Func<string, SqlTable> schema,
        Func<string, IAsyncEnumerable<SqlRow>> scan, Func<string, long?>? rowCount = null)
    {
        if (query.HasQuerySources)
        {
            long bytes = 0;
            return await QueryRelationsAsync(query, async name =>
            {
                var table = schema(name);
                var rows = new List<SqlRow>();
                await foreach (var row in scan(name))
                {
                    bytes += 128 + row.Values.Sum(p => 96L + 2L * (p.Value.Text?.Length ?? 0));
                    if (bytes > 128L * 1024 * 1024) throw new SqlException("Query inputs exceed 128 MiB.");
                    rows.Add(row);
                }
                return (table, rows);
            });
        }
        if (query.Table is null)
        {
            var empty = new SqlTable("", []);
            return Select(BindSingleSource(query, empty), empty, [new("", new(StringComparer.OrdinalIgnoreCase))]);
        }

        long retained = 0;
        void Retain(List<SqlRow> list, SqlRow row)
        {
            retained += 128 + row.Values.Sum(p => 96L + 2L * (p.Value.Text?.Length ?? 0));
            if (retained > 128L * 1024 * 1024) throw new SqlException("書庫クエリの中間結果が128 MiBを超えました。");
            list.Add(row);
        }

        if (query.Join is not null)
        {
            return await SelectJoinAsync(query, async name =>
            {
                var source = schema(name);
                var rows = new List<SqlRow>();
                await foreach (var row in scan(name)) Retain(rows, row);
                return (source, rows);
            });
        }

        var table = schema(query.Table);
        var bound = BindSingleSource(query, table);
        var unfiltered = bound with { Where = null };
        var countOnly = bound.Projections is [{ Count: true }] && bound.GroupBy is not { Count: > 0 } && bound.Having is null;
        var selectedRows = new List<SqlRow>();
        long count = 0;
        // Validate even for empty input or LIMIT 0.
        Select(bound, table, [], describeOnly: true);
        if (countOnly && bound.Where is null && rowCount?.Invoke(query.Table) is long knownCount)
            count = knownCount;
        else if (bound.Limit != 0)
        {
            await foreach (var row in scan(query.Table))
            {
                if (!Matches(bound.Where, row.Values)) continue;
                count++;
                if (countOnly) continue;
                Retain(selectedRows, row);
                if (!bound.IsGrouped && !bound.Distinct && bound.Projections.All(p => p.Window is null) && bound.Order.Count == 0 && count >= bound.Offset + (bound.Limit ?? MaxRows)) break;
            }
        }
        if (countOnly)
            return new([bound.Projections[0].Alias ?? "count"], bound.Offset > 0 || bound.Limit == 0 ? [] :
                [new[] { SqlValue.Number(count.ToString(System.Globalization.CultureInfo.InvariantCulture)) }], $"{table.Name}: {count} 行")
                { ColumnTypes = ["INTEGER"] };
        return Select(unfiltered, table, selectedRows);
    }
}
