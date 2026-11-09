namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private const string TypePrefix = "_catdb_owner/sql/v1/type/";

    private async Task BindCastsAsync(IReadOnlyList<SqlCast> casts)
    {
        if (casts.Count == 0) return;
        foreach (var group in casts.GroupBy(cast => cast.Type, StringComparer.OrdinalIgnoreCase))
        {
            var type = await GetDistinctTypeAsync(group.Key) ?? throw new SqlException($"型 '{group.Key}' が存在しません。");
            foreach (var cast in group) cast.DistinctType = type;
        }
    }

    private async Task<SqlTypeDefinition?> GetDistinctTypeAsync(string name)
    {
        var bytes = await GetAsync(TypePrefix + TableId(name));
        return bytes is null ? null : Deserialize<SqlTypeDefinition>(bytes);
    }

    private async Task<SqlResult> CreateTypeAsync(CreateType create, List<(string Key, byte[]? Value)> changes)
    {
        var type = create.Definition;
        if (await GetDistinctTypeAsync(type.Name) is not null)
        {
            if (create.IfNotExists) return Empty($"型 '{type.Name}' は既に存在します。変更なし。");
            throw new SqlException($"型 '{type.Name}' は既に存在します。");
        }
        if ((await ScanAsync(TypePrefix, 256)).Count >= 256)
            throw new SqlException("独自型の上限256に達しています。");
        if (type.Check is { } check)
        {
            var expression = SqlParser.ParseCheck(check, out var casts);
            if (casts.Any(cast => !SqlTypeRules.IsBuiltIn(cast.Type)))
                throw new SqlException("A distinct type CHECK may cast only to built-in types.");
            var value = new SqlColumn("VALUE", type.BaseType, false, false, false, null, type.MaxLength);
            ValidateCheckExpression(expression, [value]);
        }
        changes.Add((TypePrefix + TableId(type.Name), Serialize(type)));
        return Empty($"型 '{type.Name}' を作成しました。");
    }

    private async Task<SqlResult> ShowTypesAsync()
    {
        var types = (await ScanAsync(TypePrefix, 256)).Select(pair => Deserialize<SqlTypeDefinition>(pair.Value))
            .OrderBy(type => type.Name, StringComparer.OrdinalIgnoreCase);
        var rows = types.Select(type => new[]
        {
            SqlValue.String(type.Name),
            SqlValue.String(type.MaxLength is { } max ? $"{type.BaseType}({max})" : type.BaseType),
            type.Check is null ? SqlValue.Null : SqlValue.String(type.Check)
        }).ToArray();
        return new(["type_name", "base_type", "check_expression"], rows, $"{rows.Length} 型")
            { ColumnTypes = ["TEXT", "TEXT", "TEXT"] };
    }

    private async Task<SqlColumn> ResolveColumnTypeAsync(SqlColumn column)
    {
        if (!SqlTypeRules.IsBuiltIn(column.Type))
        {
            var type = await GetDistinctTypeAsync(column.Type) ?? throw new SqlException($"型 '{column.Type}' が存在しません。");
            column = column with { Type = type.Name, DistinctType = type };
        }
        return column.Default is { } value ? column with { Default = Coerce(column, value) } : column;
    }

    private async Task<List<SqlColumn>> ResolveColumnTypesAsync(IEnumerable<SqlColumn> columns, bool allowUntypedNull = false)
    {
        var resolved = new List<SqlColumn>();
        foreach (var column in columns)
            resolved.Add(allowUntypedNull && column.Type == "NULL" ? column : await ResolveColumnTypeAsync(column));
        return resolved;
    }
}
