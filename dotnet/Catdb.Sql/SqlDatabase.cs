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
    public Task<SqlResult> QueryRoleViewAsync(string sql, string role) => ExecuteRoleAsync(sql, role, allowWrites: false);

    public Task<SqlResult> ExecuteRoleAsync(string sql, string role) => ExecuteRoleAsync(sql, role, allowWrites: true);

    private async Task<SqlResult> ExecuteRoleAsync(string sql, string role, bool allowWrites)
    {
        ValidateRole(role);
        var script = SqlParser.Parse(sql);
        if (allowWrites && script is [InsertRows or UpdateRows or DeleteRows])
        {
            if (inTransaction()) throw new SqlException("ロール権限での更新には専用の接続を使用してください。");
            if (script[0] is UpdateRows update && (!UsesOnlyViewFields(update.Where) || update.Assignments.Values.Any(v => !UsesOnlyViewFields(v))) ||
                script[0] is DeleteRows delete && !UsesOnlyViewFields(delete.Where))
                throw new SqlException("更新には対象のテーブルまたはビューの列を指定してください。");
            return await ExecuteStatementAsync(script[0], role);
        }
        if (script is [ShowViews])
        {
            var views = await ListRoleViewsAsync(role, allowWrites);
            return new(["view_name"], views.Select(name => new[] { SqlValue.String(name) }).ToArray(), $"{views.Count} ビュー");
        }
        return await ReadPublishedViewAsync(RequireViewSelect(script), role);
    }

    public Task<IReadOnlyList<string>> ListRoleViewsAsync(string role) => ListRoleViewsAsync(role, false);

    private async Task<IReadOnlyList<string>> ListRoleViewsAsync(string role, bool includeWrites)
    {
        ValidateRole(role);
        if (inTransaction()) throw new SqlException("公開ビューの照会には専用の接続を使用してください。");
        var views = await ReadCatalogAsync(ViewPrefix);
        return views.Where(view => GrantsSelect(view, role) ||
            (includeWrites && view.WriteRoles?.Values.Any(roles => roles.Contains(role, StringComparer.OrdinalIgnoreCase)) == true))
            .Select(view => view.Name).ToArray();
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
            !query.HasQuerySources && UsesOnlyViewFields(query.Where) && UsesOnlyViewFields(query.Having) &&
            query.Projections.All(p => UsesOnlyViewFields(p.Expression) && UsesOnlyViewFields(p.Window?.Argument)) ? query :
            throw new SqlException("許可されたビューに対するSELECTを1文だけ指定してください。");

    private static bool UsesOnlyViewFields(SqlExpression? expression)
    {
        if (expression is null or SqlField or SqlLiteral) return true;
        if (expression is not (SqlBinary or SqlIsNull or SqlUnary or SqlCast or SqlCase or SqlInList or SqlBetween or SqlAggregate))
            return false;
        var allowed = true;
        expression.MapChildren(child => { allowed &= UsesOnlyViewFields(child); return child; });
        return allowed;
    }

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

    private async Task<SqlResult> ExecuteStatementAsync(SqlStatement statement, string? requiredRole = null)
    {
        if (statement is ExecuteProcedure execute) return await ExecuteProcedureAsync(execute);
        if (statement is TransactionStatement transaction)
        {
            if (transaction.Savepoint is { } name)
            {
                if (!inTransaction()) throw new SqlException("Savepoints require an active transaction.");
                await send(new JsonObject { ["op"] = transaction.Operation, ["name"] = SqlNames.Canonical(name) });
            }
            else await SendOperationAsync(transaction.Operation);
            return Empty($"{transaction.Operation.ToUpperInvariant()} 完了");
        }
        var ownTransaction = !inTransaction();
        if (ownTransaction) await SendOperationAsync("begin");
        var wrote = false;
        try
        {
            var changes = new List<(string Key, byte[]? Value)>();
            if (statement is InsertSelect insertQuery)
            {
                var input = await ReadWriteSourceAsync(insertQuery.Query);
                statement = new InsertRows(insertQuery.Table, insertQuery.Columns, input.Rows.Select(row => row.ToList()).ToList())
                    { SourceTypes = ResultColumnTypes(input) };
            }
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
                    var privilegeBytes = await GetAsync(privilegeKey);
                    if (privilegeBytes is null)
                    {
                        if (privilege.Access.HasFlag(ViewAccess.Select)) throw new SqlException("SELECT権限の対象は既存のビューです。");
                        privilegeKey = TableKey(privilege.View);
                        privilegeBytes = await GetAsync(privilegeKey) ?? throw new SqlException("権限の対象が存在しません。");
                    }
                    var target = Deserialize<SqlTable>(privilegeBytes);
                    var updatedPrivileges = ChangePrivilege(target, privilege);
                    if (!ReferenceEquals(target, updatedPrivileges))
                        changes.Add((privilegeKey, Serialize(updatedPrivileges)));
                    result = Empty(privilege.Grant ? "権限を付与しました。" : "権限を取り消しました。");
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
                case CreateTableAs createAs:
                    result = await CreateTableAsAsync(createAs, changes);
                    break;
                case MergeRows merge:
                    result = await MergeAsync(merge, changes);
                    break;
                case CreateTable create:
                    if (!await CanCreateTableAsync(create.Table, create.IfNotExists))
                    {
                        result = Empty("テーブルは既に存在します。変更なし。"); break;
                    }
                    foreach (var column in create.Columns)
                        if (column.Default is not null) Coerce(column, column.Default);
                    var created = new SqlTable(create.Table, create.Columns, PrimaryIndex: create.Columns.Any(c => c.PrimaryKey), Checks: create.Checks,
                        CompositePrimaryKey: create.CompositePrimaryKey, Indexes: create.CompositePrimaryKey is { } keys ? [new("PRIMARY", keys[0], keys)] : null) { ForeignKeys = create.ForeignKeys };
                    ValidateChecks(created, []);
                    await ValidateOutgoingReferencesAsync(created, []);
                    changes.Add((TableKey(create.Table), Serialize(created)));
                    result = Empty($"テーブル '{create.Table}' を作成しました。");
                    break;
                case SelectRows advanced when advanced.HasQuerySources:
                    result = await QueryRelationsAsync(advanced, name => LoadJoinSourceAsync(name, baseTablesOnly: false));
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
                        RenameColumn s => s.Table, DropColumn s => s.Table, AlterColumn s => s.Table, _ => throw new SqlException("未対応のSQLです。")
                    };
                    var schemaBytes = await GetAsync(TableKey(tableName));
                    SqlTable table;
                    if (schemaBytes is null)
                    {
                        var viewBytes = await GetAsync(ViewKey(tableName));
                        if (viewBytes is not null)
                        {
                            var view = Deserialize<SqlTable>(viewBytes);
                            if (statement is SelectRows outer)
                            {
                                result = await SelectViewAsync(outer, view);
                                break;
                            }
                            CheckWritePrivilege(view, statement, requiredRole);
                            table = await ReadSourceTableAsync(view.SourceTable!);
                            statement = TranslateViewWrite(view, table, statement);
                        }
                        else if (requiredRole is not null) throw new SqlException("指定されたテーブルまたはビューは利用できません。");
                        else if (statement is DropTable { IfExists: true }) { result = Empty("テーブルは存在しません。変更なし。"); break; }
                        else throw new SqlException($"テーブル '{tableName}' は存在しません。CREATE TABLEで作成してください。");
                    }
                    else
                    {
                        table = Deserialize<SqlTable>(schemaBytes);
                        if (requiredRole is not null) CheckWritePrivilege(table, statement, requiredRole);
                    }
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
                        case AddColumn or RenameColumn or DropColumn or AlterColumn:
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
                            var columns = InsertColumns(table, insert.Columns);
                            if (insert.SourceTypes is { } types) ValidateInsertTypes(columns, types);
                            if (existingRows.Count + insert.Rows.Count > MaxRows) throw new SqlException($"初版の1テーブル上限は{MaxRows}行です。");
                            foreach (var values in insert.Rows)
                            {
                                var inserted = MakeInsertedRow(table, columns, values);
                                var (key, row) = inserted;
                                existingRows.Add(inserted); changes.Add((key, Serialize(row)));
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
                            var assignments = ResolveAssignments(table, update.Assignments);
                            var proposed = new List<SqlRow>(existingRows.Count);
                            var affectedRows = 0;
                            foreach (var row in existingRows)
                            {
                                if (!Matches(update.Where, row.Values)) { proposed.Add(row); continue; }
                                var values = new Dictionary<string, SqlValue>(row.Values, StringComparer.OrdinalIgnoreCase);
                                ApplyAssignments(table, values, assignments, row.Values);
                                proposed.Add(new(row.StorageKey, values));
                                affectedRows++;
                            }
                            await ApplyReferentialActionsAsync(table, existingRows, proposed, changes);
                            result = Empty($"{affectedRows} 行を更新しました。");
                            break;
                        case DeleteRows delete:
                            delete.Where?.Validate(table.Columns);
                            var survivors = existingRows.Where(r => !Matches(delete.Where, r.Values)).ToList();
                            await ApplyReferentialActionsAsync(table, existingRows, survivors, changes);
                            result = Empty($"{existingRows.Count - survivors.Count} 行を削除しました。");
                            break;
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
            if (body.Any(s => s is not (SelectRows or InsertRows or InsertSelect or MergeRows or UpdateRows or DeleteRows)))
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
        var columns = TableFromResult(view.Name, Select(query, source, [], describeOnly: true)).Columns;
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
        if (view.ViewQuery is null || SqlParser.Parse(view.ViewQuery) is not [SelectRows query] || query.Table is null || query.HasQuerySources ||
            !string.Equals(query.Table, view.SourceTable, StringComparison.OrdinalIgnoreCase) ||
            !query.Joins().Select(j => j.Table).SequenceEqual(view.JoinSources ?? [], StringComparer.OrdinalIgnoreCase))
            throw new SqlException("ビュー定義が不正です。");
        var visible = query.Join is null ? await SelectTableAsync(query, await ReadSourceTableAsync(query.Table))
            : await SelectJoinAsync(query, baseTablesOnly: true);
        return visible.Rows.Select(values => new SqlRow("", new Dictionary<string, SqlValue>(visible.Columns.Zip(values, KeyValuePair.Create), StringComparer.OrdinalIgnoreCase))).ToList();
    }

    private static SqlProjection[] ExpandProjections(SelectRows select, SqlTable table) =>
        select.Projections.SelectMany(x => x.Star ? table.Columns.Select(c => new SqlProjection(new SqlField(c.Name), c.Name)) : [x]).ToArray();

    private static SqlResult Select(SelectRows select, SqlTable table, List<SqlRow> source, bool describeOnly = false)
    {
        if (select.Where is { } predicate)
        {
            predicate.Validate(table.Columns);
            if (predicate.ContainsAggregate()) throw new SqlException("Aggregates are not allowed in WHERE.");
            RequireType(ExpressionType(predicate, table.Columns), "BOOLEAN");
        }
        foreach (var projection in select.Projections) projection.Expression?.Validate(table.Columns);
        foreach (var order in select.Order.Where(o => o.OutputIndex is null)) Column(table, order.Column);
        var selected = source.Where(x => Matches(select.Where, x.Values)).ToList();
        if (select.IsGrouped) return SelectGroups(select, table, selected, describeOnly);
        var projections = ExpandProjections(select, table);
        if (projections.Length > 64) throw new SqlException("結果列は64列以下にしてください。");
        if (projections.Length == 0) throw new SqlException("SELECT *にはFROMでテーブルを指定してください。");
        var types = ProjectionTypes(projections, table);
        var names = projections.Select(p => p.Alias ?? "value").ToArray();
        if (describeOnly || select.Limit == 0) return new(names, [], "0 rows") { ColumnTypes = types };
        var windows = projections.Select(p => p.Window is { } window ? WindowValues(window, selected) : null).ToArray();
        SqlValue Project(int row, int column) => windows[column] is { } values
            ? values[row] : projections[column].Expression!.Evaluate(selected[row].Values);

        var indices = Enumerable.Range(0, selected.Count);
        IOrderedEnumerable<int>? ordered = null;
        var comparer = Comparer<SqlValue>.Create(SqlValue.Compare);
        foreach (var sort in select.Order)
        {
            SqlValue Key(int i) => sort.OutputIndex is int alias ? Project(i, alias) : selected[i].Values[sort.Column];
            ordered = ordered is null
                ? (sort.Descending ? indices.OrderByDescending(Key, comparer) : indices.OrderBy(Key, comparer))
                : (sort.Descending ? ordered.ThenByDescending(Key, comparer) : ordered.ThenBy(Key, comparer));
        }
        IEnumerable<int> output = ordered ?? indices;
        if (!select.Distinct) output = output.Skip(select.Offset).Take(select.Limit ?? MaxRows);
        var values = output.Select(i => projections.Select((_, column) => Project(i, column)).ToArray());
        var page = select.Distinct ? PageProjectedRows(select, values) : values.ToArray();
        return new(names, page, $"{page.Length} 行を取得しました。") { ColumnTypes = types };
    }

    private static SqlValue[][] PageProjectedRows(SelectRows select, IEnumerable<SqlValue[]> projected)
    {
        if (select.Distinct) projected = projected.Distinct(new GroupKeyComparer());
        return projected.Skip(select.Offset).Take(select.Limit ?? MaxRows).ToArray();
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
                var tuple = ReferenceIdentity(row, columns) ?? throw new SqlException("複合主キーにはNULLを指定できません。");
                if (!tuples.Add(tuple)) throw new SqlException("複合主キーの値が重複しています。");
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
