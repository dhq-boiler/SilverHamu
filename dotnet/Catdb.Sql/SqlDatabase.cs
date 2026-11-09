using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Catdb.Sql;

public sealed partial class SqlDatabase(Func<JsonObject, Task<JsonObject>> send, Func<bool> inTransaction)
{
    private const string CatalogPrefix = "_catdb_owner/sql/v1/table/";
    private const string ViewPrefix = "_catdb_owner/sql/v1/view/";
    private const string ProcedurePrefix = "_catdb_owner/sql/v1/procedure/";
    public const int MaxRows = 4096;
    private const int MaxReadBytes = 8 * 1024 * 1024;

    public async Task<IReadOnlyList<SqlResult>> ExecuteAsync(string sql)
    {
        var statements = SqlParser.Parse(sql);
        var results = new List<SqlResult>();
        foreach (var statement in statements)
        {
            try { results.Add(await ExecuteStatementAsync(statement)); }
            catch (Exception error) when (statements.Count > 1)
            {
                throw new SqlException($"{results.Count + 1}文目で停止: {error.Message} 先行{results.Count}文は実行済みです。BEGIN外で確定した変更は残ります。");
            }
        }
        return results;
    }

    public Task<IReadOnlyList<SqlTable>> GetTablesAsync() => ReadCatalogAsync(CatalogPrefix);
    public Task<IReadOnlyList<SqlTable>> GetViewsAsync() => ReadCatalogAsync(ViewPrefix);

    // The host supplies the allowlist and owns the connection for the entire call.
    public async Task<SqlResult> QueryPublishedViewAsync(string sql, IReadOnlyCollection<string> allowedViews)
    {
        var query = RequireViewSelect(SqlParser.Parse(sql));
        if (!allowedViews.Any(allowed => SqlNames.Canonical(allowed) == SqlNames.Canonical(query.Table!)))
            throw new SqlException("許可されたビューに対するSELECTを1文だけ指定してください。");
        return await ReadPublishedViewAsync(query, null);
    }

    // The role must come from the host's authentication context, never from the SQL client.
    public async Task<SqlResult> QueryRoleViewAsync(string sql, string role)
    {
        ValidateRole(role);
        var script = SqlParser.Parse(sql);
        if (script is [ShowViews])
        {
            var views = await ListRoleViewsAsync(role);
            return new(["view_name"], views.Select(name => new[] { SqlValue.String(name) }).ToArray(), $"{views.Count} ビュー");
        }
        return await ReadPublishedViewAsync(RequireViewSelect(script), role);
    }

    public async Task<IReadOnlyList<string>> ListRoleViewsAsync(string role)
    {
        ValidateRole(role);
        if (inTransaction()) throw new SqlException("公開ビューの照会には専用の接続を使用してください。");
        var views = await ReadCatalogAsync(ViewPrefix);
        return views.Where(view => GrantsSelect(view, role)).Select(view => view.Name).ToArray();
    }

    private static void ValidateRole(string role)
    {
        if (string.IsNullOrWhiteSpace(role) || role.Length > 128 || role.Any(char.IsControl))
            throw new SqlException("ロール名が無効です。");
    }
    private static bool GrantsSelect(SqlTable view, string role) =>
        view.SelectRoles?.Contains(role, StringComparer.OrdinalIgnoreCase) == true;

    private static SelectRows RequireViewSelect(IReadOnlyList<SqlStatement> script) =>
        script is [SelectRows { Table: not null, Join: null } query] && query.AdditionalJoins is not { Count: > 0 } &&
            UsesOnlyViewFields(query.Where) && query.Projections.All(p => UsesOnlyViewFields(p.Expression)) ? query :
            throw new SqlException("許可されたビューに対するSELECTを1文だけ指定してください。");

    private static bool UsesOnlyViewFields(SqlExpression? expression) => expression switch
    {
        null or SqlField or SqlLiteral => true,
        SqlBinary binary => UsesOnlyViewFields(binary.Left) && UsesOnlyViewFields(binary.Right),
        SqlIsNull test => UsesOnlyViewFields(test.Expression),
        _ => false
    };

    private async Task<SqlResult> ReadPublishedViewAsync(SelectRows query, string? role)
    {
        if (inTransaction()) throw new SqlException("公開ビューの照会には専用の接続を使用してください。");
        await SendOperationAsync("begin");
        try
        {
            var bytes = await GetAsync(ViewKey(query.Table!)) ?? throw new SqlException("指定された公開ビューは利用できません。");
            var view = Deserialize<SqlTable>(bytes);
            if (role is not null && !GrantsSelect(view, role))
                throw new SqlException("指定された公開ビューは利用できません。");
            return await SelectViewAsync(query, view);
        }
        finally { if (inTransaction()) await SendOperationAsync("rollback"); }
    }

    private async Task<IReadOnlyList<SqlTable>> ReadCatalogAsync(string prefix)
    {
        var ownTransaction = !inTransaction();
        if (ownTransaction) await SendOperationAsync("begin");
        try { return (await ScanAsync(prefix, 256)).Select(x => Deserialize<SqlTable>(x.Value)).OrderBy(x => x.Name, StringComparer.OrdinalIgnoreCase).ToArray(); }
        finally { if (ownTransaction && inTransaction()) await SendOperationAsync("rollback"); }
    }

    private async Task<SqlResult> ExecuteStatementAsync(SqlStatement statement)
    {
        if (statement is ExecuteProcedure execute) return await ExecuteProcedureAsync(execute);
        if (statement is TransactionStatement transaction)
        {
            await SendOperationAsync(transaction.Operation);
            return Empty($"{transaction.Operation.ToUpperInvariant()} 完了");
        }
        var ownTransaction = !inTransaction();
        if (ownTransaction) await SendOperationAsync("begin");
        var wrote = false;
        try
        {
            if (statement is SelectRows { Where: SqlInSubquery subquery } outerQuery)
                statement = outerQuery with { Where = await EvaluateSubqueryAsync(subquery) };
            var changes = new List<(string Key, byte[]? Value)>();
            SqlResult result;
            switch (statement)
            {
                case CreateProcedure createProcedure:
                    var createKey = ProcedurePrefix + TableId(createProcedure.Name);
                    if (await GetAsync(createKey) is not null)
                    {
                        if (!createProcedure.IfNotExists) throw new SqlException("同名のプロシージャーがあります。");
                        result = Empty("プロシージャーは既に存在します。変更なし。"); break;
                    }
                    changes.Add((createKey, Serialize(createProcedure.Body)));
                    result = Empty($"プロシージャー '{createProcedure.Name}' を作成しました。");
                    break;
                case DropProcedure dropProcedure:
                    var dropKey = ProcedurePrefix + TableId(dropProcedure.Name);
                    if (await GetAsync(dropKey) is null)
                    {
                        if (!dropProcedure.IfExists) throw new SqlException("プロシージャーが見つかりません。");
                        result = Empty("プロシージャーは存在しません。変更なし。"); break;
                    }
                    changes.Add((dropKey, null));
                    result = Empty($"プロシージャー '{dropProcedure.Name}' を削除しました。");
                    break;
                case ViewPrivilege privilege:
                    ValidateRole(privilege.Role);
                    var privilegeKey = ViewKey(privilege.View);
                    var privilegeBytes = await GetAsync(privilegeKey) ?? throw new SqlException("SELECT権限の対象は既存のビューです。");
                    var target = Deserialize<SqlTable>(privilegeBytes);
                    var roles = new HashSet<string>(target.SelectRoles ?? [], StringComparer.OrdinalIgnoreCase);
                    var changed = privilege.Grant ? roles.Add(privilege.Role) : roles.Remove(privilege.Role);
                    if (changed)
                        changes.Add((privilegeKey, Serialize(target with { SelectRoles = roles.Order(StringComparer.OrdinalIgnoreCase).ToList() })));
                    result = Empty(privilege.Grant ? "SELECT権限を付与しました。" : "SELECT権限を取り消しました。");
                    break;
                case ShowViews:
                    var views = await GetViewsAsync();
                    result = new(["view_name", "source_table"], views.Select(x => new[] { SqlValue.String(x.Name), SqlValue.String(x.SourceTable!) }).ToArray(), $"{views.Count} ビュー");
                    break;
                case CreateView createView:
                    var viewKey = ViewKey(createView.Name);
                    if (await GetAsync(TableKey(createView.Name)) is not null) throw new SqlException("同名のテーブルがあります。");
                    if (await GetAsync(viewKey) is not null)
                    {
                        if (!createView.IfNotExists) throw new SqlException("同名のビューがあります。");
                        result = Empty("ビューは既に存在します。変更なし。"); break;
                    }
                    if ((await GetViewsAsync()).Count >= 256) throw new SqlException("ビュー数の上限256に達しています。");
                    var definition = await DescribeViewAsync(createView);
                    changes.Add((viewKey, Serialize(definition)));
                    result = Empty($"ビュー '{createView.Name}' を作成しました。");
                    break;
                case DropView dropView:
                    if (await GetAsync(ViewKey(dropView.Name)) is null)
                    {
                        if (!dropView.IfExists || await GetAsync(TableKey(dropView.Name)) is not null) throw new SqlException($"'{dropView.Name}' はビューではありません。");
                        result = Empty("ビューは存在しません。変更なし。"); break;
                    }
                    changes.Add((ViewKey(dropView.Name), null));
                    result = Empty($"ビュー '{dropView.Name}' を削除しました。");
                    break;
                case ShowTables:
                    var tables = await GetTablesAsync();
                    result = new(["table_name", "columns"], tables.Select(x => new[] { SqlValue.String(x.Name), SqlValue.Number(x.Columns.Count.ToString(CultureInfo.InvariantCulture)) }).ToArray(), $"{tables.Count} テーブル");
                    break;
                case CreateTable create:
                    if (await GetAsync(ViewKey(create.Table)) is not null) throw new SqlException("同名のビューがあります。");
                    if (await GetAsync(TableKey(create.Table)) is not null)
                    {
                        if (!create.IfNotExists) throw new SqlException($"テーブル '{create.Table}' は既に存在します。");
                        result = Empty("テーブルは既に存在します。変更なし。"); break;
                    }
                    if ((await GetTablesAsync()).Count >= 256) throw new SqlException("テーブル数の上限256に達しています。");
                    foreach (var column in create.Columns)
                        if (column.Default is not null) Coerce(column, column.Default);
                    var created = new SqlTable(create.Table, create.Columns, PrimaryIndex: create.Columns.Any(c => c.PrimaryKey), Checks: create.Checks,
                        CompositePrimaryKey: create.CompositePrimaryKey, Indexes: create.CompositePrimaryKey is { } keys ? [new("PRIMARY", keys[0], keys)] : null);
                    ValidateChecks(created, []);
                    await ValidateOutgoingReferencesAsync(created, []);
                    changes.Add((TableKey(create.Table), Serialize(created)));
                    result = Empty($"テーブル '{create.Table}' を作成しました。");
                    break;
                case SelectRows select when select.Table is null:
                    var noTable = new SqlTable("", []);
                    result = Select(BindSingleSource(select, noTable), noTable, [new("", new(StringComparer.OrdinalIgnoreCase))]);
                    break;
                case SelectRows { Join: not null } joined:
                    result = await SelectJoinAsync(joined);
                    break;
                default:
                    var tableName = statement switch
                    {
                        SelectRows s => s.Table!, InsertRows s => s.Table, UpdateRows s => s.Table,
                        DeleteRows s => s.Table, DropTable s => s.Table, ReindexTable s => s.Table,
                        CreateIndex s => s.Table, DropIndex s => s.Table, AddColumn s => s.Table,
                        RenameColumn s => s.Table, _ => throw new SqlException("未対応のSQLです。")
                    };
                    var schemaBytes = await GetAsync(TableKey(tableName));
                    if (schemaBytes is null)
                    {
                        var viewBytes = await GetAsync(ViewKey(tableName));
                        if (viewBytes is not null)
                        {
                            if (statement is not SelectRows outer) throw new SqlException($"'{tableName}' は読み取り専用ビューです。削除にはDROP VIEWを使用してください。");
                            result = await SelectViewAsync(outer, Deserialize<SqlTable>(viewBytes));
                            break;
                        }
                        if (statement is DropTable { IfExists: true }) { result = Empty("テーブルは存在しません。変更なし。"); break; }
                        throw new SqlException($"テーブル '{tableName}' は存在しません。CREATE TABLEで作成してください。");
                    }
                    var table = Deserialize<SqlTable>(schemaBytes);
                    if (statement is SelectRows lookup)
                    {
                        result = await SelectTableAsync(lookup, table);
                        break;
                    }
                    if (statement is CreateIndex createIndex)
                    {
                        result = await CreateIndexAsync(table, createIndex, changes);
                        break;
                    }
                    if (statement is DropIndex dropIndex)
                    {
                        result = await DropIndexAsync(table, dropIndex, changes);
                        break;
                    }
                    var existingRows = await LoadRowsAsync(table);
                    var primary = table.Columns.SingleOrDefault(c => c.PrimaryKey);
                    var indexChanges = new Dictionary<string, byte[]?>(StringComparer.Ordinal);
                    var secondary = table.Indexes?.Select(i => new IndexLayout(table, i)).ToArray() ?? [];
                    switch (statement)
                    {
                        case AddColumn or RenameColumn:
                            result = await AlterTableAsync(table, statement, existingRows, changes);
                            break;
                        case ReindexTable:
                            if (primary is null && table.Indexes is not { Count: > 0 }) throw new SqlException("このテーブルには索引がありません。");
                            ValidateUnique(table, existingRows);
                            if (primary is not null)
                            {
                                foreach (var entry in await ScanAsync(IndexPrefix(table.Name), MaxRows))
                                    indexChanges[entry.Key] = null;
                                foreach (var row in existingRows)
                                    indexChanges[IndexKey(table, row.Values[primary.Name])] = Encoding.UTF8.GetBytes(row.StorageKey);
                            }
                            var rebuilt = table.Indexes?.Select(i => i with { Columns = [.. i.ColumnNames] }).ToList();
                            foreach (var rebuiltIndex in rebuilt ?? [])
                            {
                                var index = new IndexLayout(table, rebuiltIndex);
                                foreach (var entry in await ScanAsync(index.Prefix, MaxRows))
                                    indexChanges[entry.Key] = null;
                                foreach (var row in existingRows)
                                    index.RecordChange(row.StorageKey, null, row.Values, indexChanges);
                            }
                            changes.Add((TableKey(table.Name), Serialize(table with { PrimaryIndex = primary is not null, Indexes = rebuilt })));
                            result = Empty($"索引を再構築しました（{existingRows.Count} 行）。");
                            break;
                        case InsertRows insert:
                            var columns = insert.Columns ?? table.Columns.Select(x => x.Name).ToList();
                            foreach (var name in columns) Column(table, name);
                            if (existingRows.Count + insert.Rows.Count > MaxRows) throw new SqlException($"初版の1テーブル上限は{MaxRows}行です。");
                            foreach (var values in insert.Rows)
                            {
                                if (values.Count != columns.Count) throw new SqlException("INSERTの列数と値の数が一致しません。");
                                var row = new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase);
                                foreach (var column in table.Columns) row[column.Name] = Coerce(column, column.Default ?? SqlValue.Null);
                                for (var i = 0; i < columns.Count; i++) { var column = Column(table, columns[i]); row[column.Name] = Coerce(column, values[i]); }
                                ValidateRow(table, row);
                                var key = RowPrefix(table.Name) + Guid.NewGuid().ToString("N");
                                existingRows.Add(new(key, row)); changes.Add((key, Serialize(row)));
                                if (table.PrimaryIndex) indexChanges[IndexKey(table, row[primary!.Name])] = Encoding.UTF8.GetBytes(key);
                                foreach (var index in secondary)
                                    index.RecordChange(key, null, row, indexChanges);
                            }
                            ValidateUnique(table, existingRows);
                            result = Empty($"{insert.Rows.Count} 行を挿入しました。");
                            ValidateChecks(table, existingRows);
                            await ValidateOutgoingReferencesAsync(table, existingRows);
                            break;
                        case UpdateRows update:
                            update.Where?.Validate(table.Columns);
                            foreach (var pair in update.Values) Coerce(Column(table, pair.Key), pair.Value);
                            foreach (var row in existingRows.Where(x => Matches(update.Where, x.Values)))
                            {
                                var oldPrimary = table.PrimaryIndex ? row.Values[primary!.Name] : null;
                                var before = secondary.Length == 0 ? null : new Dictionary<string, SqlValue>(row.Values, StringComparer.OrdinalIgnoreCase);
                                foreach (var pair in update.Values) row.Values[Column(table, pair.Key).Name] = Coerce(Column(table, pair.Key), pair.Value);
                                ValidateRow(table, row.Values); changes.Add((row.StorageKey, Serialize(row.Values)));
                                foreach (var index in secondary)
                                    index.RecordChange(row.StorageKey, before, row.Values, indexChanges);
                                if (oldPrimary is not null && SqlValue.Compare(oldPrimary, row.Values[primary!.Name]) != 0)
                                {
                                    indexChanges[IndexKey(table, oldPrimary)] = null;
                                    indexChanges[IndexKey(table, row.Values[primary.Name])] = Encoding.UTF8.GetBytes(row.StorageKey);
                                }
                            }
                            ValidateUnique(table, existingRows);
                            ValidateChecks(table, existingRows);
                            await ValidateOutgoingReferencesAsync(table, existingRows);
                            await ValidateIncomingReferencesAsync(table, existingRows, update);
                            result = Empty($"{changes.Count} 行を更新しました。");
                            break;
                        case DeleteRows delete:
                            delete.Where?.Validate(table.Columns);
                            var deletedRows = existingRows.Where(x => Matches(delete.Where, x.Values)).ToArray();
                            foreach (var row in deletedRows)
                            {
                                changes.Add((row.StorageKey, null));
                                if (table.PrimaryIndex) indexChanges[IndexKey(table, row.Values[primary!.Name])] = null;
                                foreach (var index in secondary)
                                    index.RecordChange(row.StorageKey, row.Values, null, indexChanges);
                            }
                            var deletedKeys = deletedRows.Select(r => r.StorageKey).ToHashSet(StringComparer.Ordinal);
                            existingRows.RemoveAll(r => deletedKeys.Contains(r.StorageKey));
                            await ValidateIncomingReferencesAsync(table, existingRows, null);
                            result = Empty($"{changes.Count} 行を削除しました。"); break;
                        case DropTable:
                            await RestrictReferencedSchemaAsync(table, null);
                            var dependent = (await GetViewsAsync()).FirstOrDefault(x => ViewUsesTable(x, table.Name));
                            if (dependent is not null) throw new SqlException($"先に依存ビュー '{dependent.Name}' を削除してください。");
                            foreach (var row in existingRows) changes.Add((row.StorageKey, null));
                            if (table.PrimaryIndex)
                                foreach (var entry in await ScanAsync(IndexPrefix(table.Name), MaxRows)) indexChanges[entry.Key] = null;
                            foreach (var index in secondary)
                                foreach (var entry in await ScanAsync(index.Prefix, MaxRows)) indexChanges[entry.Key] = null;
                            changes.Add((TableKey(table.Name), null));
                            result = Empty($"テーブル '{table.Name}' を削除しました。"); break;
                        default: throw new SqlException("未対応のSQLです。");
                    }
                    foreach (var (key, value) in indexChanges) changes.Add((key, value));
                    break;
            }
            if (changes.Sum(x => (long)(x.Value?.Length ?? 0) + Encoding.UTF8.GetByteCount(x.Key)) > MaxReadBytes)
                throw new SqlException("1文の変更量が8 MiBを超えています。分割してください。");
            foreach (var change in changes)
            {
                wrote = true;
                var request = new JsonObject { ["op"] = change.Value is null ? "delete" : "put", ["key"] = SqlNames.Hex(change.Key) };
                if (change.Value is not null) request["value"] = Convert.ToHexString(change.Value);
                await send(request);
            }
            if (ownTransaction)
                await SendOperationAsync(changes.Count > 0 ? "commit" : "rollback");
            else if (changes.Count > 0) result = result with { Message = result.Message + " 未確定です。COMMITで確定、ROLLBACKで取消。" };
            return result;
        }
        catch (Exception error)
        {
            if ((ownTransaction || wrote) && inTransaction())
            {
                try { await SendOperationAsync("rollback"); }
                catch (Exception rollbackError) { throw new SqlException($"{error.Message} 取消にも失敗しました: {rollbackError.Message}。接続を閉じて状態を確認してください。"); }
                if (wrote && !ownTransaction) throw new SqlException($"{error.Message} 書込失敗のため、明示トランザクション全体を取り消しました。");
            }
            throw;
        }
    }

    private async Task<SqlResult> ExecuteProcedureAsync(ExecuteProcedure procedure)
    {
        var ownTransaction = !inTransaction();
        if (ownTransaction) await SendOperationAsync("begin");
        var executing = false;
        try
        {
            var bytes = await GetAsync(ProcedurePrefix + TableId(procedure.Name))
                ?? throw new SqlException($"プロシージャー '{procedure.Name}' は存在しません。");
            var body = SqlParser.Parse(Deserialize<string>(bytes));
            if (body.Any(s => s is not (SelectRows or InsertRows or UpdateRows or DeleteRows)))
                throw new SqlException("保存されたプロシージャーに対応外の文があります。");
            executing = true;
            SqlResult last = Empty("");
            foreach (var command in body) last = await ExecuteStatementAsync(command);
            if (ownTransaction)
                await SendOperationAsync(body.All(s => s is SelectRows) ? "rollback" : "commit");
            var message = $"プロシージャー '{procedure.Name}' の{body.Count}文を実行しました。";
            if (!ownTransaction) message += " 明示トランザクション内です。";
            return last with { Message = message };
        }
        catch (Exception error)
        {
            if ((ownTransaction || executing) && inTransaction())
            {
                try { await SendOperationAsync("rollback"); }
                catch (Exception rollbackError) { throw new SqlException($"{error.Message} 取消にも失敗しました: {rollbackError.Message}"); }
                if (!ownTransaction) throw new SqlException($"{error.Message} 明示トランザクション全体を取り消しました。");
            }
            throw;
        }
    }

    private async Task<SqlTable> ReadSourceTableAsync(string name)
    {
        var bytes = await GetAsync(TableKey(name)) ?? throw new SqlException($"元テーブル '{name}' が見つかりません。");
        return Deserialize<SqlTable>(bytes);
    }

    private static bool ViewUsesTable(SqlTable view, string table) =>
        string.Equals(view.SourceTable, table, StringComparison.OrdinalIgnoreCase) ||
        view.JoinSources?.Contains(table, StringComparer.OrdinalIgnoreCase) == true;

    private async Task<SqlTable> DescribeViewAsync(CreateView view, SqlTable? replacement = null)
    {
        var tables = new List<SqlTable>();
        foreach (var name in view.Select.Joins().Select(j => j.Table).Prepend(view.Select.Table!))
            tables.Add(replacement is not null && replacement.Name.Equals(name, StringComparison.OrdinalIgnoreCase)
                ? replacement : await ReadSourceTableAsync(name));
        var source = tables[0];
        SelectRows query;
        if (view.Select.Join is null) query = BindSingleSource(view.Select, source);
        else
        {
            var plan = BindJoin(view.Select, tables);
            query = plan.Query;
            source = plan.Schema;
        }
        Select(query, source, []);
        var columns = new List<SqlColumn>();
        foreach (var projection in ExpandProjections(query, source))
        {
            var type = projection.Aggregate.HasValue ? "REAL" : projection.Expression switch
            {
                SqlField field => Column(source, field.Name).Type,
                SqlLiteral literal => literal.Value.Kind == "NUMBER" ? "REAL" : literal.Value.Kind,
                _ => "INTEGER" // COUNT(*)
            };
            var name = projection.Alias!;
            if (columns.Any(x => x.Name.Equals(name, StringComparison.OrdinalIgnoreCase))) throw new SqlException($"ビューの列名 '{name}' が重複しています。ASで別名を指定してください。");
            columns.Add(new(name, type, false, false, false, null, null));
        }
        return new(view.Name, columns, view.Query, tables[0].Name, JoinSources: tables.Skip(1).Select(t => t.Name).ToList());
    }

    private async Task<SqlResult> SelectViewAsync(SelectRows outer, SqlTable view)
    {
        var bound = BindSingleSource(outer, view);
        return Select(bound, view, await LoadViewRowsAsync(view));
    }

    private async Task<(SqlTable Table, List<SqlRow> Rows)> LoadJoinSourceAsync(string name, bool baseTablesOnly)
    {
        var schema = await GetAsync(TableKey(name));
        if (schema is not null)
        {
            var table = Deserialize<SqlTable>(schema);
            return (table, await LoadRowsAsync(table));
        }
        if (baseTablesOnly) throw new SqlException($"ビュー定義の元テーブル '{name}' がありません。");
        var bytes = await GetAsync(ViewKey(name)) ?? throw new SqlException($"テーブルまたはビュー '{name}' は存在しません。");
        var view = Deserialize<SqlTable>(bytes);
        return (view, await LoadViewRowsAsync(view));
    }

    private async Task<List<SqlRow>> LoadViewRowsAsync(SqlTable view)
    {
        if (view.ViewQuery is null || SqlParser.Parse(view.ViewQuery) is not [SelectRows query] || query.Table is null || query.Where is SqlInSubquery ||
            !string.Equals(query.Table, view.SourceTable, StringComparison.OrdinalIgnoreCase) ||
            !query.Joins().Select(j => j.Table).SequenceEqual(view.JoinSources ?? [], StringComparer.OrdinalIgnoreCase))
            throw new SqlException("ビュー定義が不正です。");
        var visible = query.Join is null ? await SelectTableAsync(query, await ReadSourceTableAsync(query.Table))
            : await SelectJoinAsync(query, baseTablesOnly: true);
        return visible.Rows.Select(values => new SqlRow("", new Dictionary<string, SqlValue>(visible.Columns.Zip(values, KeyValuePair.Create), StringComparer.OrdinalIgnoreCase))).ToList();
    }

    private static SqlProjection[] ExpandProjections(SelectRows select, SqlTable table) =>
        select.Projections.SelectMany(x => x.Star ? table.Columns.Select(c => new SqlProjection(new SqlField(c.Name), c.Name)) : [x]).ToArray();

    private static SqlResult Select(SelectRows select, SqlTable table, List<SqlRow> source)
    {
        select.Where?.Validate(table.Columns);
        foreach (var projection in select.Projections) projection.Expression?.Validate(table.Columns);
        foreach (var order in select.Order.Where(o => o.OutputIndex is null)) Column(table, order.Column);
        var selected = source.Where(x => Matches(select.Where, x.Values)).ToList();
        if (select.IsGrouped) return SelectGroups(select, table, selected);
        if (select.Order.Count > 0)
            selected.Sort((left, right) =>
            {
                foreach (var order in select.Order)
                {
                    var comparison = SqlValue.Compare(left.Values[order.Column], right.Values[order.Column]);
                    if (comparison != 0) return order.Descending ? -comparison : comparison;
                }
                return 0;
            });
        var projections = ExpandProjections(select, table);
        if (projections.Length > 64) throw new SqlException("結果列は64列以下にしてください。");
        if (projections.Length == 0) throw new SqlException("SELECT *にはFROMでテーブルを指定してください。");
        var page = selected.Skip(select.Offset).Take(select.Limit ?? MaxRows)
            .Select(row => projections.Select(p => p.Expression!.Evaluate(row.Values)).ToArray()).ToArray();
        return new(projections.Select(x => x.Alias ?? "value").ToArray(), page, $"{page.Length} 行を取得しました。");
    }

    private static bool Matches(SqlExpression? expression, Dictionary<string, SqlValue> row)
    {
        if (expression is null) return true;
        var value = expression.Evaluate(row);
        if (!value.IsNull && value.Kind != "BOOLEAN") throw new SqlException("WHEREには条件式を指定してください。");
        return value.IsTrue;
    }
    private static SqlColumn Column(SqlTable table, string name) => table.Columns.FirstOrDefault(x => x.Name.Equals(name, StringComparison.OrdinalIgnoreCase)) ?? throw new SqlException($"列 '{name}' は存在しません。");
    private static SqlValue Coerce(SqlColumn column, SqlValue value)
    {
        if (value.IsNull) return value;
        var compatible = column.Type switch
        {
            "INTEGER" => value.Kind == "NUMBER" && decimal.TryParse(value.Text, CultureInfo.InvariantCulture, out var n) && n == decimal.Truncate(n) && n >= long.MinValue && n <= long.MaxValue,
            "REAL" => value.Kind == "NUMBER", "TEXT" => value.Kind == "TEXT", "BOOLEAN" => value.Kind == "BOOLEAN", "BLOB" => value.Kind == "BLOB", _ => false
        };
        if (!compatible) throw new SqlException($"列 '{column.Name}' の型は{column.Type}です。値の型が一致しません。");
        if (column.MaxLength is int max && (value.Kind == "BLOB" ? value.Text!.Length / 2 : value.Text!.Length) > max) throw new SqlException($"列 '{column.Name}' は長さ{max}以下にしてください。");
        return value;
    }
    private static void ValidateRow(SqlTable table, Dictionary<string, SqlValue> row)
    {
        foreach (var column in table.Columns)
        {
            Coerce(column, row[column.Name]);
            if (column.NotNull && row[column.Name].IsNull) throw new SqlException($"列 '{column.Name}' はNULLにできません。");
        }
    }
    private static void ValidateUnique(SqlTable table, List<SqlRow> rows)
    {
        if (table.CompositePrimaryKey is { } columns)
        {
            var tuples = new HashSet<string>(StringComparer.Ordinal);
            foreach (var row in rows)
            {
                var tuple = new StringBuilder();
                foreach (var column in columns)
                {
                    var part = UniqueIdentity(row.Values[column]);
                    tuple.Append(part.Length).Append(':').Append(part);
                }
                if (!tuples.Add(tuple.ToString())) throw new SqlException("複合主キーの値が重複しています。");
            }
        }
        foreach (var column in table.Columns.Where(x => x.Unique))
        {
            var values = new HashSet<string>(StringComparer.Ordinal);
            foreach (var row in rows)
            {
                var value = row.Values[column.Name]; if (value.IsNull) continue;
                if (!values.Add(UniqueIdentity(value))) throw new SqlException($"列 '{column.Name}' の値が重複しています（PRIMARY KEY / UNIQUE）。");
            }
        }
    }
    internal static string UniqueIdentity(SqlValue value) => value.Kind == "NUMBER"
        ? decimal.Parse(value.Text!, CultureInfo.InvariantCulture).ToString("G29", CultureInfo.InvariantCulture) : value.Text!;

    private static string IndexPrefix(string table) => "_catdb_owner/sql/v1/pk/" + TableId(table) + "/";
    private static string IndexKey(SqlTable table, SqlValue value) =>
        IndexPrefix(table.Name) + Digest(UniqueIdentity(value));

    private async Task<SqlResult> SelectTableAsync(SelectRows query, SqlTable table)
    {
        query = BindSingleSource(query, table);
        var equality = query.Where as SqlBinary;
        var operands = (equality?.Left, equality?.Right) switch
        {
            (SqlField f, SqlLiteral l) => (Field: f, Literal: l),
            (SqlLiteral l, SqlField f) => (Field: f, Literal: l),
            _ => default
        };
        if (equality?.Operator == "=" && operands.Field is not null)
        {
            var column = Column(table, operands.Field.Name);
            var literal = operands.Literal!.Value;
            var kind = column.Type is "INTEGER" or "REAL" ? "NUMBER" : column.Type;
            if (table.PrimaryIndex && column.PrimaryKey && kind == literal.Kind)
            {
                var rows = new List<SqlRow>();
                var pointer = await GetAsync(IndexKey(table, literal));
                if (pointer is not null)
                {
                    var key = Encoding.UTF8.GetString(pointer);
                    if (!key.StartsWith(RowPrefix(table.Name), StringComparison.Ordinal))
                        throw new SqlException("主キー索引の参照先が不正です。");
                    var bytes = await GetAsync(key) ?? throw new SqlException("主キー索引の参照先がありません。");
                    var row = DecodeRow(key, bytes);
                    if (!Matches(query.Where, row.Values)) throw new SqlException("主キー索引と行の値が一致しません。");
                    rows.Add(row);
                }
                return Select(query, table, rows);
            }
        }
        if (FindIndexRange(query.Where, table) is { } range)
            return await SelectSecondaryAsync(query, table, range);
        return Select(query, table, await LoadRowsAsync(table));
    }

    private static SqlRow DecodeRow(string key, byte[] value) =>
        new(key, new(Deserialize<Dictionary<string, SqlValue>>(value), StringComparer.OrdinalIgnoreCase));
    private async Task<List<SqlRow>> LoadRowsAsync(SqlTable table)
    {
        return (await ScanAsync(RowPrefix(table.Name), MaxRows)).Select(x => DecodeRow(x.Key, x.Value)).ToList();
    }
    private Task<List<(string Key, byte[] Value)>> ScanAsync(string prefix, int maxRows) =>
        ScanRangeAsync(prefix, prefix + "~", maxRows);

    private async Task<List<(string Key, byte[] Value)>> ScanRangeAsync(string start, string end, int maxRows)
    {
        var rows = new List<(string, byte[])>(); string? after = null; var totalBytes = 0; var pages = 0;
        do
        {
            if (++pages > 256) throw new SqlException("走査ページ数の上限に達しました。");
            var request = new JsonObject { ["op"] = "scan", ["start"] = SqlNames.Hex(start), ["end"] = SqlNames.Hex(end), ["rows"] = 256, ["bytes"] = 524288, ["visits"] = 65536 };
            if (after is not null) request["after"] = after;
            var page = await send(request);
            foreach (var item in page["rows"]!.AsArray())
            {
                var key = Encoding.UTF8.GetString(Convert.FromHexString(item![0]!.ToString())); var value = Convert.FromHexString(item[1]!.ToString());
                totalBytes += value.Length;
                if (rows.Count >= maxRows || totalBytes > MaxReadBytes) throw new SqlException($"読込上限（{maxRows}行・8 MiB）を超えています。部分結果は返しません。");
                rows.Add((key, value));
            }
            var next = page["next_after"]?.ToString();
            if (next is not null && string.Equals(next, after, StringComparison.OrdinalIgnoreCase)) throw new SqlException("走査カーソルが進みません。");
            after = next;
        } while (after is not null);
        return rows;
    }
    private async Task<byte[]?> GetAsync(string key)
    {
        var response = await send(new JsonObject { ["op"] = "get", ["key"] = SqlNames.Hex(key) });
        return response["value"] is JsonNode value ? Convert.FromHexString(value.ToString()) : null;
    }
    private Task<JsonObject> SendOperationAsync(string op) => send(new JsonObject { ["op"] = op });
    private static string TableId(string table) => Digest(SqlNames.Canonical(table));
    private static string TableKey(string table) => CatalogPrefix + TableId(table);
    private static string ViewKey(string view) => ViewPrefix + TableId(view);
    private static string RowPrefix(string table) => "_catdb_owner/sql/v1/row/" + TableId(table) + "/";
    private static byte[] Serialize<T>(T value)
    {
        var bytes = JsonSerializer.SerializeToUtf8Bytes(value);
        if (bytes.Length > 256 * 1024) throw new SqlException("1行またはテーブル定義の保存サイズは256 KiB以下にしてください。");
        return bytes;
    }
    private static T Deserialize<T>(byte[] bytes) => JsonSerializer.Deserialize<T>(bytes) ?? throw new SqlException("SQLデータの形式が不正です。");
    private static SqlResult Empty(string message) => new([], [], message);
}
