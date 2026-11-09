namespace Catdb.Sql;

public sealed partial class SqlParser
{
    private SqlExpressionBindings bindings = new();

    private SqlExpression Expression(int depth = 0)
    {
        CheckDepth(depth);
        var left = And(depth);
        while (Match("OR")) left = new SqlBinary(left, "OR", And(depth));
        return left;
    }

    private static void CheckDepth(int depth)
    {
        if (depth > 32) throw new SqlException("SQL expression nesting exceeds 32 levels.");
    }

    private SqlExpression And(int depth)
    {
        var left = Condition(depth);
        while (Match("AND")) left = new SqlBinary(left, "AND", Condition(depth));
        return left;
    }

    private SqlExpression Condition(int depth)
    {
        CheckDepth(depth);
        if (Match("NOT")) return new SqlUnary("NOT", Condition(depth + 1));
        var left = Additive(depth);
        if (Match("IS"))
        {
            var negated = Match("NOT");
            Expect("NULL");
            return new SqlIsNull(left, negated);
        }

        var negate = Match("NOT");
        SqlExpression predicate;
        if (Match("BETWEEN"))
        {
            var lower = Additive(depth);
            Expect("AND");
            predicate = new SqlBetween(left, lower, Additive(depth));
        }
        else if (Match("IN"))
        {
            Expect("(");
            if (Is("SELECT") || Is("WITH")) predicate = MembershipSubquery(left);
            else
            {
                var values = new List<SqlExpression>();
                do { values.Add(Expression(depth + 1)); } while (Match(","));
                Expect(")");
                predicate = new SqlInList(left, values);
            }
        }
        else if (Match("LIKE")) predicate = new SqlBinary(left, "LIKE", Additive(depth));
        else
        {
            if (negate) throw Error("Expected BETWEEN, IN or LIKE after NOT.");
            if (Peek.Kind == "symbol" && Peek.Text is "=" or "<>" or "!=" or "<=" or ">=" or "<" or ">")
                return new SqlBinary(left, tokens[current++].Text, Additive(depth));
            return left;
        }
        return negate ? new SqlUnary("NOT", predicate) : predicate;
    }

    private SqlExpression Additive(int depth)
    {
        var left = Multiplicative(depth);
        while (Is("+") || Is("-"))
        {
            var sum = new SqlBinary(left, tokens[current++].Text, Multiplicative(depth));
            if (sum.Operator == "+" && MayHaveDistinctType(sum.Left) && MayHaveDistinctType(sum.Right))
                bindings.Additions.Add(sum);
            left = sum;
        }
        return left;
    }

    private static bool MayHaveDistinctType(SqlExpression operand) => operand switch
    {
        SqlLiteral literal => literal.Value.IsNull,
        SqlCast cast => !SqlTypeRules.IsBuiltIn(cast.Type),
        _ => true
    };

    private SqlExpression Multiplicative(int depth)
    {
        var left = Factor(depth);
        while (Is("*") || Is("/") || Is("%")) left = new SqlBinary(left, tokens[current++].Text, Factor(depth));
        return left;
    }

    private SqlExpression Factor(int depth)
    {
        CheckDepth(depth);
        if (++conditionCount > 512) throw Error("A statement may contain at most 512 expression operands.");
        if (Is("+") || Is("-"))
        {
            if (current + 1 < tokens.Count && tokens[current + 1].Kind == "number")
                return new SqlLiteral(Literal());
            return new SqlUnary(tokens[current++].Text, Factor(depth + 1));
        }
        if (Is("EXISTS") || IsQueryParenthesis) return ParseSubqueryAtom();
        if (Match("("))
        {
            var expression = Expression(depth + 1);
            Expect(")");
            return expression;
        }
        if (QualifiedStart) return Field();
        if (Match("CASE"))
        {
            var operand = Is("WHEN") ? null : Expression(depth + 1);
            var branches = new List<SqlWhen>();
            do
            {
                Expect("WHEN"); var condition = Expression(depth + 1);
                Expect("THEN"); branches.Add(new(condition, Expression(depth + 1)));
            } while (Is("WHEN"));
            var fallback = Match("ELSE") ? Expression(depth + 1) : null;
            Expect("END");
            return new SqlCase(operand, branches, fallback);
        }
        if (Match("CAST"))
        {
            Expect("("); var argument = Expression(depth + 1); Expect("AS");
            var (type, length) = ColumnType(); Expect(")");
            var cast = new SqlCast(argument, type, length);
            if (!SqlTypeRules.IsBuiltIn(type)) bindings.NamedCasts.Add(cast);
            return cast;
        }
        if (Peek.Kind == "word" && current + 1 < tokens.Count && tokens[current + 1] is { Kind: "symbol", Text: "(" } &&
            (Is("COUNT") || Is("SUM") || Is("AVG") || Is("MIN") || Is("MAX")))
        {
            var function = Name().ToUpperInvariant();
            Expect("(");
            var distinct = Match("DISTINCT");
            if (!distinct) Match("ALL");
            var argument = function == "COUNT" && !distinct && Match("*") ? null : Expression(depth + 1);
            Expect(")");
            if (argument?.ContainsAggregate() == true) throw Error("Aggregate functions cannot be nested.");
            return new SqlAggregate(function, argument, distinct);
        }
        return Peek.Kind is "string" or "number" || Is("NULL") || Is("TRUE") || Is("FALSE") ||
               (Is("X") || Is("N")) && current + 1 < tokens.Count && tokens[current + 1].Kind == "string"
            ? new SqlLiteral(Literal()) : Field();
    }
}
