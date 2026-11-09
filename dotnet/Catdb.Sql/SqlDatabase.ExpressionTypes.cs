namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    internal static string ExpressionType(SqlExpression expression, IReadOnlyList<SqlColumn> columns)
    {
        string TypeOf(SqlExpression item) => ExpressionType(item, columns);
        switch (expression)
        {
            case SqlLiteral literal:
                return literal.Value.Kind == "NUMBER"
                    ? long.TryParse(literal.Value.Text, System.Globalization.CultureInfo.InvariantCulture, out _) ? "INTEGER" : "REAL"
                    : literal.Value.Kind;
            case SqlField field:
                return columns.FirstOrDefault(c => c.Name.Equals(field.Name, StringComparison.OrdinalIgnoreCase))?.Type
                    ?? throw new SqlException($"Unknown column '{field.Name}'.");
            case SqlBoundQuery query: return query.ResultType;
            case SqlIsNull test:
                TypeOf(test.Expression);
                return "BOOLEAN";
            case SqlBetween range:
                var rangeType = CommonType(TypeOf(range.Value), TypeOf(range.Lower));
                CommonType(rangeType, TypeOf(range.Upper));
                return "BOOLEAN";
            case SqlInList list:
                var comparison = TypeOf(list.Value);
                foreach (var item in list.Items) comparison = CommonType(comparison, TypeOf(item));
                return "BOOLEAN";
            case SqlUnary unary:
                var operand = TypeOf(unary.Operand);
                if (unary.Operator == "NOT") { RequireType(operand, "BOOLEAN"); return "BOOLEAN"; }
                RequireNumber(operand);
                return operand;
            case SqlBinary binary:
                var left = TypeOf(binary.Left); var right = TypeOf(binary.Right);
                if (binary.Operator is "+" or "-" or "*" or "/" or "%")
                {
                    RequireNumber(left); RequireNumber(right);
                    return binary.Operator == "/" ? "REAL" : CommonType(left, right);
                }
                if (binary.Operator is "AND" or "OR") { RequireType(left, "BOOLEAN"); RequireType(right, "BOOLEAN"); }
                else if (binary.Operator == "LIKE") { RequireType(left, "TEXT"); RequireType(right, "TEXT"); }
                else CommonType(left, right);
                return "BOOLEAN";
            case SqlCast cast:
                TypeOf(cast.Operand);
                return cast.Type;
            case SqlCase conditional:
                var caseType = conditional.Else is null ? "NULL" : TypeOf(conditional.Else);
                var key = conditional.Operand is null ? null : TypeOf(conditional.Operand);
                foreach (var branch in conditional.Branches)
                {
                    var conditionType = TypeOf(branch.When);
                    if (key is null) RequireType(conditionType, "BOOLEAN");
                    else CommonType(key, conditionType);
                    caseType = CommonType(caseType, TypeOf(branch.Then));
                }
                return caseType;
            case SqlAggregate aggregate:
                var argument = aggregate.Argument is null ? "NULL" : TypeOf(aggregate.Argument);
                if (aggregate.Function == "COUNT") return "INTEGER";
                if (aggregate.Function is "MIN" or "MAX") return argument;
                RequireNumber(argument);
                return "REAL";
            default: throw new SqlException("Cannot determine the type of this expression.");
        }
    }

    private static void RequireType(string actual, string expected)
    {
        if (actual != "NULL" && actual != expected)
            throw new SqlException($"Expected {expected}, received {actual}.");
    }

    private static void RequireNumber(string type)
    {
        if (type is not ("INTEGER" or "REAL" or "NULL")) throw new SqlException($"Expected a number, received {type}.");
    }

    private static string CommonType(string first, string second)
    {
        if (first == second || second == "NULL") return first;
        if (first == "NULL") return second;
        if (first is "INTEGER" or "REAL" && second is "INTEGER" or "REAL") return "REAL";
        throw new SqlException($"Incompatible SQL types {first} and {second}.");
    }
}
