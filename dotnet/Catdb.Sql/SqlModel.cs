using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;
using System.Text.Json.Serialization;

namespace Catdb.Sql;

public sealed class SqlException(string message) : Exception(message);

public sealed record SqlValue(string Kind, string? Text)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? TypeName { get; init; }
    public static readonly SqlValue Null = new("NULL", null);
    public static SqlValue String(string text) => new("TEXT", text);
    public static SqlValue Number(string text)
    {
        if (!SqlTypeRules.TryParseExactNumber(text, out var value))
            throw new SqlException("数値が範囲外です。");
        return new("NUMBER", value.ToString(CultureInfo.InvariantCulture));
    }
    public static SqlValue Boolean(bool value) => new("BOOLEAN", value ? "true" : "false");
    [JsonIgnore] public bool IsNull => Kind == "NULL";
    [JsonIgnore] public bool IsTrue => Kind == "BOOLEAN" && Text == "true";
    [JsonIgnore] public string Display => IsNull ? "NULL" : Kind == "BLOB" ? $"0x{Text}" : Text!;
    public static int Compare(SqlValue left, SqlValue right)
    {
        if (left.IsNull || right.IsNull) return left.IsNull ? right.IsNull ? 0 : -1 : 1;
        if (left.Kind != right.Kind || !string.Equals(left.TypeName, right.TypeName, StringComparison.OrdinalIgnoreCase))
            throw new SqlException($"異なる型 {left.TypeName ?? left.Kind} と {right.TypeName ?? right.Kind} は比較できません。");
        return left.Kind switch
        {
            "NUMBER" => decimal.Parse(left.Text!, CultureInfo.InvariantCulture).CompareTo(decimal.Parse(right.Text!, CultureInfo.InvariantCulture)),
            "DURATION" => SqlTypeRules.DurationTicks(left).CompareTo(SqlTypeRules.DurationTicks(right)),
            _ => string.CompareOrdinal(left.Text, right.Text)
        };
    }
}

public enum SqlReferenceAction { Restrict, Cascade, SetNull, SetDefault }
public sealed record SqlReference(string Table, string Column, bool DeleteCascade = false, bool UpdateCascade = false)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public SqlReferenceAction? OnDelete { get; init; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public SqlReferenceAction? OnUpdate { get; init; }
}
public sealed record SqlForeignKey(List<string> Columns, string Table, List<string> Targets, bool DeleteCascade, bool UpdateCascade)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public SqlReferenceAction? OnDelete { get; init; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public SqlReferenceAction? OnUpdate { get; init; }
    internal SqlReference SingleReference() => new(Table, Targets.Single(), DeleteCascade, UpdateCascade)
        { OnDelete = OnDelete, OnUpdate = OnUpdate };
}
public sealed record SqlTypeDefinition(string Name, string BaseType, int? MaxLength, string? Check);
public sealed record SqlColumn(string Name, string Type, bool NotNull, bool Unique, bool PrimaryKey, SqlValue? Default, int? MaxLength, SqlReference? Reference = null, List<string>? Checks = null)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public SqlTypeDefinition? DistinctType { get; init; }
}
public sealed record SqlIndex(string Name, string Column, List<string>? Columns = null)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingDefault)] public bool Unique { get; init; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)] public string? Predicate { get; init; }
    // A missing list identifies the original single-column hash index.
    [JsonIgnore] public IReadOnlyList<string> ColumnNames => Columns ?? [Column];
}
public sealed record SqlTable(string Name, List<SqlColumn> Columns, string? ViewQuery = null, string? SourceTable = null, bool PrimaryIndex = false, List<SqlIndex>? Indexes = null, List<string>? Checks = null, List<string>? SelectRoles = null, List<string>? JoinSources = null, List<string>? CompositePrimaryKey = null)
{
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public List<SqlTrigger>? Triggers { get; init; }
    public Dictionary<string, List<string>>? WriteRoles { get; init; }
    public List<SqlForeignKey>? ForeignKeys { get; init; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public SqlTableStatistics? Statistics { get; init; }
}
public sealed record SqlRow(string StorageKey, Dictionary<string, SqlValue> Values);
public sealed record SqlTableStatistics(long RowCount, Dictionary<string, SqlColumnStatistics> Columns);
public sealed record SqlColumnStatistics(long NonNullRows, long DistinctCount, Dictionary<string, long> FrequentValues);
public sealed record AnalyzeTable(string Name) : SqlStatement;
public sealed record SqlResult(IReadOnlyList<string> Columns, IReadOnlyList<SqlValue[]> Rows, string Message)
{
    public IReadOnlyList<string>? ColumnTypes { get; init; }
}

public abstract record SqlStatement
{
    internal ConflictAction? Conflict { get; init; }
    internal List<SqlProjection>? Returning { get; init; }
    internal SqlExpression? ReturningFilter { get; init; }
    internal SqlExpressionBindings? ExpressionBindings { get; init; }
}

internal sealed class SqlExpressionBindings
{
    internal List<SqlCast> NamedCasts { get; } = [];
    internal List<SqlBinary> Additions { get; } = [];
}
public sealed record CreateType(SqlTypeDefinition Definition, bool IfNotExists) : SqlStatement;
public sealed record EnableAddition(string TypeName) : SqlStatement;
public sealed record ShowTypes : SqlStatement;
public sealed record ExplainQuery(SelectRows Query, bool Analyze = false) : SqlStatement;
public sealed record CreateTable(string Table, List<SqlColumn> Columns, bool IfNotExists, List<string>? Checks = null, List<string>? CompositePrimaryKey = null, List<SqlForeignKey>? ForeignKeys = null) : SqlStatement;
public sealed record DropTable(string Table, bool IfExists) : SqlStatement;
public sealed record AddColumn(string Table, SqlColumn Column) : SqlStatement;
public sealed record RenameColumn(string Table, string Column, string NewName) : SqlStatement;
public sealed record DropColumn(string Table, string Column) : SqlStatement;
public enum SqlColumnAlteration { Type, SetDefault, DropDefault, SetNotNull, DropNotNull }
public sealed record AlterColumn(string Table, string Column, SqlColumnAlteration Operation,
    string? Type = null, int? MaxLength = null, SqlValue? Default = null) : SqlStatement;
public sealed record ReindexTable(string Table) : SqlStatement;
public sealed record CreateIndex(string Name, string Table, List<string> Columns, bool IfNotExists) : SqlStatement
{
    public bool Unique { get; init; }
    public string? Predicate { get; init; }
}
public sealed record DropIndex(string Name, string Table, bool IfExists) : SqlStatement;
public abstract record InsertStatement(string Table, List<string>? Columns) : SqlStatement
{
    internal abstract int RowCount { get; }
}
public sealed record InsertRows(string Table, List<string>? Columns, List<List<SqlValue>> Rows) : InsertStatement(Table, Columns)
{
    internal override int RowCount => Rows.Count;
    internal IReadOnlyList<string>? SourceTypes { get; init; }
}
public sealed record InsertValues(string Table, List<string>? Columns, List<List<SqlExpression>> Rows) : InsertStatement(Table, Columns)
{
    internal override int RowCount => Rows.Count;
}
public sealed record InsertSelect(string Table, List<string>? Columns, SelectRows Query) : SqlStatement;
public sealed record CreateTableAs(string Table, List<string>? Columns, SelectRows Query, bool IfNotExists) : SqlStatement;
public sealed partial record SelectRows(string? Table, List<SqlProjection> Projections, SqlExpression? Where, List<SqlOrder> Order, int? Limit, int Offset, string? TableAlias = null, SqlJoin? Join = null, IReadOnlyList<SqlExpression>? GroupBy = null, List<SqlJoin>? AdditionalJoins = null) : SqlStatement
{
    public bool Distinct { get; init; }
    public SqlExpression? Having { get; init; }
    internal string? GroupDescription { get; init; }
    public bool IsGrouped => Having is not null || GroupBy is { Count: > 0 } ||
        Projections.Any(p => p.IsAggregate || p.Window?.Argument?.ContainsAggregate() == true || p.Window?.Filter?.ContainsAggregate() == true);
    public IEnumerable<SqlJoin> Joins()
    {
        if (Join is not null) yield return Join;
        if (AdditionalJoins is not null)
            foreach (var join in AdditionalJoins) yield return join;
    }
}
public enum SqlJoinKind { Inner, Left, Right, Full, Cross }
public sealed record SqlJoin(string Table, string? Alias, SqlField? Left, SqlField? Right, SqlJoinKind Kind = SqlJoinKind.Inner)
{
    public SqlExpression? On { get; init; }
    public List<string>? Using { get; init; }
    public bool Natural { get; init; }
}
public sealed record UpdateRows(string Table, Dictionary<string, SqlValue> Values, SqlExpression? Where) : SqlStatement
{
    private Dictionary<string, SqlExpression>? expressions;
    public IReadOnlyDictionary<string, SqlExpression> Assignments =>
        expressions ?? Values.ToDictionary(p => p.Key, p => (SqlExpression)new SqlLiteral(p.Value), StringComparer.OrdinalIgnoreCase);

    internal static UpdateRows FromAssignments(string table, Dictionary<string, SqlExpression> assignments, SqlExpression? where)
    {
        var literals = assignments.Where(p => p.Value is SqlLiteral)
            .ToDictionary(p => p.Key, p => ((SqlLiteral)p.Value).Value, StringComparer.OrdinalIgnoreCase);
        return new(table, literals, where) { expressions = literals.Count == assignments.Count ? null : assignments };
    }
}
public sealed record DeleteRows(string Table, SqlExpression? Where) : SqlStatement;
public sealed record TransactionStatement(string Operation, string? Savepoint = null) : SqlStatement;
public sealed record ShowTables : SqlStatement;
public sealed record LockRows(SelectRows Query) : SqlStatement;
public sealed record ShowViews : SqlStatement;
[Flags]
public enum ViewAccess { Select = 1, Insert = 2, Update = 4, Delete = 8 }
public sealed record ViewPrivilege(string View, string Role, bool Grant) : SqlStatement
{
    public ViewAccess Access { get; init; } = ViewAccess.Select;
}
public sealed record CreateProcedure(string Name, string Body, bool IfNotExists) : SqlStatement;
public sealed record ExecuteProcedure(string Name) : SqlStatement;
public sealed record DropProcedure(string Name, bool IfExists) : SqlStatement;
public sealed record CreateView(string Name, SelectRows Select, string Query, bool IfNotExists) : SqlStatement;
public sealed record DropView(string Name, bool IfExists) : SqlStatement;
public enum NumericAggregate { Sum, Average }
public sealed record SqlProjection(SqlExpression? Expression, string? Alias, bool Star = false, bool Count = false, string? Qualifier = null, NumericAggregate? Aggregate = null)
{
    public SqlWindow? Window { get; init; }
    public bool IsAggregate => Window is null && (Count || Aggregate.HasValue || Expression?.ContainsAggregate() == true);
}
public sealed record SqlOrder(string Column, bool Descending, string? Qualifier = null, int? OutputIndex = null)
{
    public bool? NullsFirst { get; init; }
    internal string Description => $"{Column} {(Descending ? "DESC" : "ASC")}" +
        (NullsFirst is bool first ? first ? " NULLS FIRST" : " NULLS LAST" : "");

    internal int Compare(SqlValue left, SqlValue right)
    {
        if (left.IsNull != right.IsNull)
            return left.IsNull == (NullsFirst ?? !Descending) ? -1 : 1;
        int comparison = SqlValue.Compare(left, right);
        return Descending ? -comparison : comparison;
    }
}

public abstract record SqlExpression
{
    public SqlExpression Rewrite(Func<SqlExpression, SqlExpression?> replace) =>
        replace(this) ?? MapChildren(child => child.Rewrite(replace));

    internal bool ContainsAggregate()
    {
        bool found = false;
        Rewrite(node => { if (node is SqlAggregate) found = true; return found ? node : null; });
        return found;
    }

    public abstract SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row);
    public virtual SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) => this;
    public virtual void Validate(IEnumerable<SqlColumn> columns) =>
        MapChildren(child => { child.Validate(columns); return child; });
}
public sealed record SqlLiteral(SqlValue Value) : SqlExpression
{
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) => Value;
}
public sealed record SqlField(string Name, string? Qualifier = null) : SqlExpression
{
    public override void Validate(IEnumerable<SqlColumn> columns)
    {
        if (Qualifier is not null) throw new SqlException($"列 '{Qualifier}.{Name}' の参照先を解決できません。");
        if (!columns.Any(x => x.Name.Equals(Name, StringComparison.OrdinalIgnoreCase))) throw new SqlException($"列 '{Name}' は存在しません。");
    }
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) => row[Name];
}
public sealed record SqlBinary(SqlExpression Left, string Operator, SqlExpression Right) : SqlExpression
{
    internal IReadOnlyDictionary<string, SqlTypeDefinition>? AdditionTypes { get; set; }

    internal SqlTypeDefinition ResolveDistinctAddition(string left, string right)
    {
        var name = left == "NULL" ? right : left;
        if ((left == "NULL" || right == "NULL" || left.Equals(right, StringComparison.OrdinalIgnoreCase))
            && AdditionTypes is not null && AdditionTypes.TryGetValue(name, out var definition))
            return definition;
        throw new SqlException($"Addition is not enabled for {left} and {right}.");
    }

    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) => this with { Left = map(Left), Right = map(Right) };
    public override void Validate(IEnumerable<SqlColumn> columns) { Left.Validate(columns); Right.Validate(columns); }
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var left = Left.Evaluate(row); var right = Right.Evaluate(row);
        if (Operator is "AND" or "OR")
        {
            if ((!left.IsNull && left.Kind != "BOOLEAN") || (!right.IsNull && right.Kind != "BOOLEAN")) throw new SqlException("AND / ORには条件式が必要です。");
            if (Operator == "AND") return (!left.IsNull && !left.IsTrue) || (!right.IsNull && !right.IsTrue) ? SqlValue.Boolean(false) : left.IsNull || right.IsNull ? SqlValue.Null : SqlValue.Boolean(true);
            return left.IsTrue || right.IsTrue ? SqlValue.Boolean(true) : left.IsNull || right.IsNull ? SqlValue.Null : SqlValue.Boolean(false);
        }
        if (Operator == "+" && (left.TypeName is not null || right.TypeName is not null))
        {
            var type = ResolveDistinctAddition(left.TypeName ?? left.Kind, right.TypeName ?? right.Kind);
            return left.IsNull || right.IsNull ? SqlValue.Null : SqlTypeRules.AddDistinct(left, right, type);
        }
        if (left.IsNull || right.IsNull) return SqlValue.Null;
        if (Operator is "+" or "-" or "*" or "/" or "%")
            return SqlNumbers.Calculate(left, Operator, right);
        if (Operator == "LIKE")
        {
            if (left.Kind != "TEXT" || right.Kind != "TEXT") throw new SqlException("LIKEはTEXT列に使用してください。");
            var pattern = "\\A" + Regex.Escape(right.Text!).Replace("%", ".*").Replace("_", ".") + "\\z";
            return SqlValue.Boolean(Regex.IsMatch(left.Text!, pattern, RegexOptions.Singleline | RegexOptions.CultureInvariant, TimeSpan.FromMilliseconds(100)));
        }
        var comparison = SqlValue.Compare(left, right);
        return SqlValue.Boolean(Operator switch
        {
            "=" => comparison == 0, "<>" or "!=" => comparison != 0,
            "<" => comparison < 0, "<=" => comparison <= 0, ">" => comparison > 0, ">=" => comparison >= 0,
            _ => throw new SqlException("未対応の比較演算子です。")
        });
    }
}
public sealed record SqlIsNull(SqlExpression Expression, bool Negated) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) => this with { Expression = map(Expression) };
    public override void Validate(IEnumerable<SqlColumn> columns) => Expression.Validate(columns);
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) => SqlValue.Boolean(Expression.Evaluate(row).IsNull != Negated);
}

public static class SqlNames
{
    public static string Canonical(string name) => name.ToLowerInvariant();
    public static string Quote(string name) => "[" + name.Replace("]", "]]") + "]";
    public static string Hex(string text) => Convert.ToHexString(Encoding.UTF8.GetBytes(text));
}
