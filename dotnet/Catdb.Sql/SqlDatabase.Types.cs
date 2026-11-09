namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private const string TypePrefix = "_catdb_owner/sql/v1/type/";
    private const string AdditionPrefix = "_catdb_owner/sql/v1/operator/add/";
    private const int MaxTypes = 256;

    private Task<List<(string Key, byte[] Value)>> ScanTypeCatalogAsync(string prefix) =>
        ScanRangeAsync(prefix, prefix + "~", MaxTypes, MaxTypes * MaxSerializedBytes);

    private async Task<IReadOnlyDictionary<string, SqlTypeDefinition>> GetAdditionTypesAsync()
    {
        var entries = await ScanTypeCatalogAsync(AdditionPrefix);
        return entries.Select(entry => Deserialize<SqlTypeDefinition>(entry.Value))
            .ToDictionary(type => type.Name, StringComparer.OrdinalIgnoreCase);
    }

    private async Task<SqlResult> EnableAdditionAsync(EnableAddition addition, List<(string Key, byte[]? Value)> changes)
    {
        var type = await GetDistinctTypeAsync(addition.TypeName)
            ?? throw new SqlException($"加算を有効にする独自型 '{addition.TypeName}' が存在しません。");
        if (!IsNumericType(type.BaseType))
            throw new SqlException($"型 '{type.Name}' の加算には数値の基底型が必要です。");
        var key = AdditionPrefix + TableId(type.Name);
        if (await GetAsync(key) is not null)
            throw new SqlException($"型 '{type.Name}' の加算は既に有効です。");
        changes.Add((key, Serialize(type)));
        return Empty($"型 '{type.Name}' の加算を有効にしました。");
    }

    private async Task BindExpressionsAsync(SqlExpressionBindings? bindings)
    {
        if (bindings is null) return;
        foreach (var group in bindings.NamedCasts.GroupBy(cast => cast.Type, StringComparer.OrdinalIgnoreCase))
        {
            var type = await GetDistinctTypeAsync(group.Key) ?? throw new SqlException($"型 '{group.Key}' が存在しません。");
            foreach (var cast in group) cast.DistinctType = type;
        }
        if (bindings.Additions.Count > 0)
        {
            var types = await GetAdditionTypesAsync();
            foreach (var addition in bindings.Additions) addition.AdditionTypes = types;
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
        if ((await ScanTypeCatalogAsync(TypePrefix)).Count >= MaxTypes)
            throw new SqlException("独自型の上限256に達しています。");
        if (type.Check is { } check)
        {
            var expression = SqlParser.ParseCheck(check, out var bindings);
            if (bindings.NamedCasts.Count > 0)
                throw new SqlException("A distinct type CHECK may cast only to built-in types.");
            var value = new SqlColumn("VALUE", type.BaseType, false, false, false, null, type.MaxLength);
            ValidateCheckExpression(expression, [value]);
        }
        changes.Add((TypePrefix + TableId(type.Name), Serialize(type)));
        return Empty($"型 '{type.Name}' を作成しました。");
    }

    private async Task<SqlResult> ShowTypesAsync()
    {
        var types = (await ScanTypeCatalogAsync(TypePrefix)).Select(pair => Deserialize<SqlTypeDefinition>(pair.Value))
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
