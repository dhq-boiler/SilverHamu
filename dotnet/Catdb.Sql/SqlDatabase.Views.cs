namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private static SqlTable ChangePrivilege(SqlTable target, ViewPrivilege privilege)
    {
        foreach (var access in Enum.GetValues<ViewAccess>())
        {
            if (!privilege.Access.HasFlag(access)) continue;
            var key = access.ToString();
            var roles = access == ViewAccess.Select ? target.SelectRoles : target.WriteRoles?.GetValueOrDefault(key);
            var updated = new HashSet<string>(roles ?? [], StringComparer.OrdinalIgnoreCase);
            var changed = privilege.Grant ? updated.Add(privilege.Role) : updated.Remove(privilege.Role);
            if (!changed) continue;
            var names = updated.Order(StringComparer.OrdinalIgnoreCase).ToList();
            if (access == ViewAccess.Select)
                target = target with { SelectRoles = names };
            else
            {
                var writes = new Dictionary<string, List<string>>(target.WriteRoles ?? []);
                if (names.Count == 0) writes.Remove(key);
                else writes[key] = names;
                target = target with { WriteRoles = writes };
            }
        }
        return target;
    }

    private static void CheckWritePrivilege(SqlTable target, SqlStatement statement, string? role)
    {
        var access = statement switch
        {
            InsertStatement => ViewAccess.Insert,
            UpdateRows => ViewAccess.Update,
            DeleteRows => ViewAccess.Delete,
            _ => throw new SqlException($"'{target.Name}' に対してこの操作はできません。")
        };
        if (role is not null &&
            target.WriteRoles?.GetValueOrDefault(access.ToString())?.Contains(role, StringComparer.OrdinalIgnoreCase) != true)
            throw new SqlException("指定されたテーブルまたはビューは利用できません。");
    }

    private async Task<SqlStatement> TranslateViewWriteAsync(SqlTable view, SqlTable source, SqlStatement statement)
    {
        if (view.ViewQuery is null || SqlParser.Parse(view.ViewQuery) is not [SelectRows definition] ||
            !string.Equals(definition.Table, source.Name, StringComparison.OrdinalIgnoreCase) ||
            definition.Join is not null || definition.IsGrouped || definition.Distinct || definition.HasQuerySources ||
            definition.Projections.Any(p => p.Window is not null) ||
            definition.Limit is not null || definition.Offset != 0)
            throw new SqlException($"ビュー '{view.Name}' は更新できません。");
        await BindExpressionsAsync(definition.ExpressionBindings);
        var bound = BindSingleSource(definition, source);
        var columns = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var sources = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var projection in bound.Projections)
        {
            if (projection.Expression is not SqlField field || !sources.Add(field.Name))
                throw new SqlException($"ビュー '{view.Name}' の各列は異なるベーステーブル列を参照する必要があります。");
            columns.Add(projection.Alias!, field.Name);
        }
        string SourceColumn(string name) => columns.TryGetValue(name, out var column) ? column :
            throw new SqlException($"ビュー '{view.Name}' に列 '{name}' はありません。");
        SqlExpression Translate(SqlExpression expression) => expression.Rewrite(node =>
        {
            if (node is not SqlField field) return null;
            if (field.Qualifier is not null && !string.Equals(field.Qualifier, view.Name, StringComparison.OrdinalIgnoreCase))
                throw new SqlException($"列 '{field.Qualifier}.{field.Name}' の参照先を解決できません。");
            return new SqlField(SourceColumn(field.Name));
        });
        SqlExpression? Filter(SqlExpression? where)
        {
            if (where is null) return bound.Where;
            var requested = Translate(where);
            // Evaluate the caller's predicate only for rows passing the published view filter.
            return bound.Where is null ? requested :
                new SqlCase(null, [new(bound.Where, requested)], new SqlLiteral(SqlValue.Boolean(false)));
        }
        return statement switch
        {
            InsertStatement insert => insert with { Table = source.Name, Columns = (insert.Columns ?? view.Columns.Select(c => c.Name).ToList()).Select(SourceColumn).ToList() },
            UpdateRows update => UpdateRows.FromAssignments(source.Name,
                update.Assignments.ToDictionary(p => SourceColumn(p.Key), p => Translate(p.Value), StringComparer.OrdinalIgnoreCase), Filter(update.Where)),
            DeleteRows delete => delete with { Table = source.Name, Where = Filter(delete.Where) },
            _ => throw new SqlException("未対応のビュー更新です。")
        };
    }
}
