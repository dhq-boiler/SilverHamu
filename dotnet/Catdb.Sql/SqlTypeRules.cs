using System.Globalization;
using System.Numerics;
using System.Runtime.CompilerServices;

namespace Catdb.Sql;

internal static class SqlTypeRules
{
    private static readonly CultureInfo Invariant = CultureInfo.InvariantCulture;
    private static readonly string[] ClockFormats = ["HH:mm", "HH:mm:ss", "HH:mm:ss.FFFFFFF"];
    private static readonly string[] LocalFormats = ["yyyy-MM-dd'T'HH:mm", "yyyy-MM-dd'T'HH:mm:ss", "yyyy-MM-dd'T'HH:mm:ss.FFFFFFF"];
    private static readonly string[] InstantFormats = LocalFormats.SelectMany(format => new[] { format + "'Z'", format + "zzz" }).ToArray();
    private static readonly ConditionalWeakTable<SqlTypeDefinition, SqlExpression> TypeChecks = new();

    public static bool IsDecimal(string type) => TryDecimalParameters(type, out _, out _);

    public static bool IsBuiltIn(string type) => type is "INTEGER" or "REAL" or "BOOLEAN" or "TEXT" or "BLOB" or "UUID"
        or "DATE" or "TIME" or "LOCAL_DATETIME" or "INSTANT" or "DURATION" || IsDecimal(type);

    private static bool TryDecimalParameters(string type, out int precision, out int scale)
    {
        precision = scale = 0;
        if (!type.StartsWith("DECIMAL(", StringComparison.Ordinal) || !type.EndsWith(')')) return false;
        var parameters = type.AsSpan(8, type.Length - 9);
        var comma = parameters.IndexOf(',');
        return comma > 0 && int.TryParse(parameters[..comma], NumberStyles.None, Invariant, out precision)
            && int.TryParse(parameters[(comma + 1)..], NumberStyles.None, Invariant, out scale)
            && precision is >= 1 and <= 28 && scale <= precision;
    }

    internal static bool TryParseExactNumber(string text, out decimal number)
    {
        if (!decimal.TryParse(text, NumberStyles.Float, Invariant, out number)) return false;
        // Decimal.TryParse can silently discard significant digits near the precision limit.
        var exponent = text.AsSpan().IndexOfAny('e', 'E');
        if (text.Length <= 28 && exponent < 0) return true;
        var mantissa = exponent < 0 ? text : text[..exponent];
        static string SignificantDigits(string digits) =>
            digits.Trim().TrimStart('+', '-').Replace(".", "").Trim('0');
        return SignificantDigits(mantissa) == SignificantDigits(number.ToString(Invariant));
    }

    internal static SqlValue AddDistinct(SqlValue left, SqlValue right, SqlTypeDefinition definition)
    {
        if (left.Kind != "NUMBER" || right.Kind != "NUMBER")
            throw new SqlException($"Operator + requires numeric operands for {definition.Name}.");
        // Decimal addition can round a large coefficient to fit, even in a checked context.
        var (x, xScale) = DecimalParts(left.Text!);
        var (y, yScale) = DecimalParts(right.Text!);
        var scale = Math.Max(xScale, yScale);
        var sum = x * BigInteger.Pow(10, scale - xScale) + y * BigInteger.Pow(10, scale - yScale);
        var exact = FormattableString.Invariant($"{sum}e-{scale}");
        if (!TryParseExactNumber(exact, out var number))
            throw new SqlException($"Sum cannot be represented exactly as {definition.Name}.");
        return Assign(new("NUMBER", number.ToString(Invariant)), definition.Name, definition.MaxLength, definition);
    }

    private static (BigInteger Coefficient, int Scale) DecimalParts(string text)
    {
        var point = text.IndexOf('.');
        var scale = point < 0 ? 0 : text.Length - point - 1;
        var digits = point < 0 ? text : text.Remove(point, 1);
        return (BigInteger.Parse(digits, Invariant), scale);
    }

    public static SqlValue Assign(SqlValue value, string type, int? length, SqlTypeDefinition? definition)
    {
        if (value.TypeName is { } name && !string.Equals(name, definition?.Name, StringComparison.OrdinalIgnoreCase))
            throw new SqlException($"Cannot assign {name} to {type}.");
        var raw = value.TypeName is null ? value : value with { TypeName = null };
        var result = raw.IsNull ? SqlValue.Null :
            ConvertBase(raw, definition?.BaseType ?? type, definition?.MaxLength ?? length, explicitCast: false);
        if (definition is null) return result;
        if (definition.Check is { } check)
        {
            var row = new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase) { ["VALUE"] = result };
            var predicate = TypeChecks.GetValue(definition, static type => SqlParser.ParseCheck(type.Check!));
            var passed = predicate.Evaluate(row);
            if (!passed.IsNull && !passed.IsTrue)
                throw new SqlException($"Value violates {definition.Name} CHECK ({check}).");
        }
        return result.IsNull ? result : result with { TypeName = definition.Name };
    }

    public static SqlValue Cast(SqlValue value, string type, int? length, SqlTypeDefinition? definition)
    {
        if (definition is not null) return Assign(value, type, length, definition);
        if (value.IsNull) return value;
        if (value.TypeName is { } name)
        {
            var kind = type is "INTEGER" or "REAL" || IsDecimal(type) ? "NUMBER" : type;
            if (value.Kind != kind) throw new SqlException($"Cast {name} to its base type first.");
            return ConvertBase(value with { TypeName = null }, type, length, explicitCast: false);
        }
        return ConvertBase(value, type, length, explicitCast: true);
    }

    private static SqlValue ConvertBase(SqlValue value, string type, int? length, bool explicitCast)
    {
        var text = value.Text!;
        if (type == "TEXT" && (explicitCast || value.Kind == "TEXT"))
            return CheckLength(SqlValue.String(text), length);
        if (type == "BLOB" && (value.Kind == "BLOB" || explicitCast && value.Kind == "TEXT"))
        {
            if (value.Kind == "BLOB") return CheckLength(value, length);
            try { return CheckLength(new("BLOB", Convert.ToHexString(Convert.FromHexString(text))), length); }
            catch (FormatException) { throw new SqlException("CAST to BLOB requires hexadecimal text."); }
        }
        if (type == "BOOLEAN" && (value.Kind == "BOOLEAN" || explicitCast && value.Kind == "TEXT")
            && bool.TryParse(text, out var boolean))
            return SqlValue.Boolean(boolean);
        if (type is "INTEGER" or "REAL" || IsDecimal(type))
        {
            if ((value.Kind == "NUMBER" || explicitCast && value.Kind == "TEXT") && TryParseExactNumber(text, out var number))
                return ConvertNumber(number, type, explicitCast);
        }
        else if (value.Kind == "TEXT" || value.Kind == type)
        {
            switch (type)
            {
                case "UUID" when Guid.TryParseExact(text, "D", out var uuid) && text.Length == 36:
                    return new(type, uuid.ToString("D"));
                case "DATE" when DateOnly.TryParseExact(text, "yyyy-MM-dd", Invariant, DateTimeStyles.None, out var date):
                    return new(type, date.ToString("yyyy-MM-dd", Invariant));
                case "TIME" when TimeOnly.TryParseExact(text, ClockFormats, Invariant, DateTimeStyles.None, out var time):
                    return new(type, time.ToString("HH:mm:ss.fffffff", Invariant));
                case "LOCAL_DATETIME" when DateTime.TryParseExact(text, LocalFormats, Invariant, DateTimeStyles.None, out var local):
                    return new(type, local.ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff", Invariant));
                case "INSTANT" when DateTimeOffset.TryParseExact(text, InstantFormats, Invariant,
                        DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out var instant):
                    return new(type, instant.ToString("yyyy-MM-dd'T'HH:mm:ss.fffffff'Z'", Invariant));
                case "DURATION" when TimeSpan.TryParseExact(text, "c", Invariant, out var duration):
                    return new(type, duration.ToString("c", Invariant));
            }
        }
        throw new SqlException($"Invalid {type} value (source type {value.Kind}).");
    }

    private static SqlValue ConvertNumber(decimal number, string type, bool explicitCast)
    {
        if (type == "INTEGER")
        {
            if (explicitCast) number = decimal.Truncate(number);
            if (number != decimal.Truncate(number) || number < long.MinValue || number > long.MaxValue)
                throw new SqlException("INTEGER requires a signed 64-bit integer.");
        }
        if (TryDecimalParameters(type, out int precision, out int scale))
        {
            decimal limit = 1;
            for (int i = 0; i < precision - scale; i++) limit *= 10;
            if (decimal.Abs(number) >= limit || decimal.Round(number, scale) != number)
                throw new SqlException($"Value cannot be represented exactly as {type}.");
            return new("NUMBER", number.ToString($"F{scale}", Invariant));
        }
        return new("NUMBER", number.ToString(Invariant));
    }

    private static SqlValue CheckLength(SqlValue value, int? length)
    {
        if (length is { } max && (value.Kind == "BLOB" ? value.Text!.Length / 2 : value.Text!.Length) > max)
            throw new SqlException($"Value exceeds length {max}.");
        return value;
    }

    internal static long DurationTicks(SqlValue value) => TimeSpan.ParseExact(value.Text!, "c", Invariant).Ticks;
}
