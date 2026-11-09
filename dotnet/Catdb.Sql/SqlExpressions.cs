using System.Globalization;
using System.Numerics;

namespace Catdb.Sql;

internal static class SqlNumbers
{
    internal static decimal Numeric(SqlValue value) => value.Kind == "NUMBER" && value.TypeName is null
        ? decimal.Parse(value.Text!, CultureInfo.InvariantCulture)
        : throw new SqlException("Arithmetic requires numeric operands.");

    internal static SqlValue Number(decimal value) => SqlValue.Number(value.ToString(CultureInfo.InvariantCulture));

    internal static BigInteger UnitsAtScale28(decimal value)
    {
        Span<int> bits = stackalloc int[4];
        decimal.GetBits(value, bits);
        var coefficient = ((BigInteger)(uint)bits[2] << 64) + ((BigInteger)(uint)bits[1] << 32) + (uint)bits[0];
        int scale = (bits[3] >> 16) & 255;
        var magnitude = coefficient * BigInteger.Pow(10, 28 - scale);
        return bits[3] < 0 ? -magnitude : magnitude;
    }

    internal static SqlValue Calculate(SqlValue left, string op, SqlValue right)
    {
        var x = Numeric(left); var y = Numeric(right);
        try
        {
            return Number(op switch
            {
                "+" => x + y, "-" => x - y, "*" => x * y, "/" => x / y, "%" => x % y,
                _ => throw new SqlException($"Unknown arithmetic operation '{op}'.")
            });
        }
        catch (OverflowException) { throw new SqlException("Arithmetic result is outside the decimal range."); }
        catch (DivideByZeroException) { throw new SqlException("Division by zero."); }
    }
}

public sealed record SqlUnary(string Operator, SqlExpression Operand) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) => this with { Operand = map(Operand) };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var value = Operand.Evaluate(row);
        if (value.IsNull) return SqlValue.Null;
        return Operator switch
        {
            "NOT" when value.Kind == "BOOLEAN" => SqlValue.Boolean(!value.IsTrue),
            "+" => SqlNumbers.Number(SqlNumbers.Numeric(value)),
            "-" => SqlNumbers.Number(-SqlNumbers.Numeric(value)),
            _ => throw new SqlException($"Invalid operand for {Operator}.")
        };
    }
}

public sealed record SqlBetween(SqlExpression Value, SqlExpression Lower, SqlExpression Upper) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) =>
        this with { Value = map(Value), Lower = map(Lower), Upper = map(Upper) };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var value = Value.Evaluate(row); var lower = Lower.Evaluate(row); var upper = Upper.Evaluate(row);
        if (value.IsNull) return SqlValue.Null;
        if (!lower.IsNull && SqlValue.Compare(value, lower) < 0 || !upper.IsNull && SqlValue.Compare(value, upper) > 0)
            return SqlValue.Boolean(false);
        return lower.IsNull || upper.IsNull ? SqlValue.Null : SqlValue.Boolean(true);
    }
}

public sealed record SqlInList(SqlExpression Value, IReadOnlyList<SqlExpression> Items) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) =>
        this with { Value = map(Value), Items = Items.Select(map).ToArray() };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var value = Value.Evaluate(row);
        if (value.IsNull) return SqlValue.Null;
        var containsNull = false;
        foreach (var item in Items)
        {
            var candidate = item.Evaluate(row);
            if (candidate.IsNull) containsNull = true;
            else if (SqlValue.Compare(value, candidate) == 0) return SqlValue.Boolean(true);
        }
        return containsNull ? SqlValue.Null : SqlValue.Boolean(false);
    }
}

public sealed record SqlWhen(SqlExpression When, SqlExpression Then);
public sealed record SqlCase(SqlExpression? Operand, IReadOnlyList<SqlWhen> Branches, SqlExpression? Else) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) =>
        this with { Operand = Operand is null ? null : map(Operand), Else = Else is null ? null : map(Else),
            Branches = Branches.Select(b => new SqlWhen(map(b.When), map(b.Then))).ToArray() };

    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var operand = Operand?.Evaluate(row);
        foreach (var branch in Branches)
        {
            var condition = branch.When.Evaluate(row);
            if (operand is null && !condition.IsNull && condition.Kind != "BOOLEAN")
                throw new SqlException("CASE WHEN requires a boolean condition.");
            if (operand is null ? condition.IsTrue : !operand.IsNull && !condition.IsNull && SqlValue.Compare(operand, condition) == 0)
                return branch.Then.Evaluate(row);
        }
        return Else?.Evaluate(row) ?? SqlValue.Null;
    }
}

public sealed record SqlCast(SqlExpression Operand, string Type, int? MaxLength = null) : SqlExpression
{
    internal SqlTypeDefinition? DistinctType { get; set; }
    internal string TargetType => DistinctType?.Name ?? (SqlTypeRules.IsBuiltIn(Type)
        ? Type : throw new SqlException($"Unknown SQL type '{Type}'."));
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) => this with { Operand = map(Operand) };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row)
    {
        var value = Operand.Evaluate(row);
        return SqlTypeRules.Cast(value, TargetType, MaxLength, DistinctType);
    }
}

public sealed record SqlAggregate(string Function, SqlExpression? Argument, bool Distinct = false) : SqlExpression
{
    public SqlExpression? Filter { get; init; }
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) =>
        this with { Argument = Argument is null ? null : map(Argument), Filter = Filter is null ? null : map(Filter) };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) =>
        throw new SqlException($"{Function} requires an aggregate context.");
}
