namespace Catdb.Sql;

internal sealed record SqlCommonTable(string Name, List<string>? Columns, SelectRows Query);
internal sealed record SqlDerivedTable(SelectRows Query, List<string>? Columns);
internal sealed record SqlSetExpression(SelectRows Left, string Operation, SelectRows Right);
internal enum SubqueryKind { Scalar, Exists, In }

internal sealed record SqlNestedQuery(SelectRows Query, SubqueryKind Kind, SqlExpression? Operand = null) : SqlExpression
{
    public override SqlExpression MapChildren(Func<SqlExpression, SqlExpression> map) =>
        this with { Operand = Operand is null ? null : map(Operand) };
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) =>
        throw new SqlException("A subquery requires a SELECT context.");

    internal static bool Contains(SqlExpression? expression)
    {
        bool found = false;
        expression?.Rewrite(node => { found |= node is SqlNestedQuery; return null; });
        return found;
    }
}

// The subquery closure binds outer column names while the enclosing query binds its own fields.
internal sealed record SqlBoundQuery(string ResultType, Func<IReadOnlyDictionary<string, SqlValue>, SqlValue> Run) : SqlExpression
{
    internal IReadOnlyList<string> CorrelatedFields { get; init; } = [];
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) => Run(row);
}

public sealed partial record SelectRows
{
    internal IReadOnlyList<SqlCommonTable> CommonTables { get; init; } = [];
    internal IReadOnlyDictionary<string, SqlDerivedTable>? DerivedSources { get; init; }
    internal bool Recursive { get; init; }
    internal SqlSetExpression? SetExpression { get; init; }
    internal bool HasQuerySources => CommonTables.Count != 0 || SetExpression is not null ||
        DerivedSources?.Count > 0 || Expressions().Any(SqlNestedQuery.Contains);

    internal IEnumerable<SqlExpression?> Expressions() =>
        Projections.Select(p => p.Expression).Concat(Projections.Select(p => p.Window?.Argument))
            .Concat(Projections.Select(p => p.Window?.Filter)).Append(Where).Append(Having);
}

public sealed partial class SqlParser
{
    private int queryDepth;
    private int compoundCount;
    private int derivedNumber;

    private bool IsQueryParenthesis => Is("(") && current + 1 < tokens.Count &&
        tokens[current + 1].Kind == "word" &&
        (tokens[current + 1].Text.Equals("SELECT", StringComparison.OrdinalIgnoreCase) ||
         tokens[current + 1].Text.Equals("WITH", StringComparison.OrdinalIgnoreCase));

    private SelectRows QueryExpression()
    {
        if (++queryDepth > 32) throw Error("Subqueries may be nested at most 32 levels.");
        try
        {
            var definitions = new List<SqlCommonTable>();
            bool recursive = false;
            if (Match("WITH"))
            {
                recursive = Match("RECURSIVE");
                do
                {
                    var name = Name();
                    if (definitions.Count == 64 || definitions.Any(d => d.Name.Equals(name, StringComparison.OrdinalIgnoreCase)))
                        throw Error("A WITH clause requires up to 64 distinct table names.");
                    var columns = OptionalColumnList();
                    Expect("AS"); Expect("(");
                    var query = QueryExpression();
                    Expect(")");
                    definitions.Add(new(name, columns, query));
                } while (Match(","));
            }
            var result = QueryTail(SetOperands(0));
            return definitions.Count == 0 ? result : result with { CommonTables = definitions, Recursive = recursive };
        }
        finally { queryDepth--; }
    }

    private SelectRows SetOperands(int minimumPrecedence)
    {
        SelectRows left;
        if (Match("(")) { left = QueryExpression(); Expect(")"); }
        else if (Match("VALUES")) left = ValuesOperand();
        else { Expect("SELECT"); left = SelectCore(); }
        while (true)
        {
            int precedence = Is("INTERSECT") ? 2 : Is("UNION") || Is("EXCEPT") ? 1 : 0;
            if (precedence <= minimumPrecedence) return left;
            if (++compoundCount > 128) throw Error("Too many set operations in one statement.");
            var op = tokens[current++].Text.ToUpperInvariant();
            if (Match("ALL")) op += " ALL";
            else Match("DISTINCT");
            var right = SetOperands(precedence);
            left = new(null, [], null, [], null, 0) { SetExpression = new(left, op, right) };
        }
    }

    private SelectRows ValuesOperand()
    {
        // A row constructor has the same projection and type rules as a single-row SELECT.
        var rows = new List<SelectRows>();
        do
        {
            Expect("(");
            var values = new List<SqlProjection>();
            do
            {
                if (values.Count == 64) throw Error("A VALUES row may contain at most 64 columns.");
                values.Add(new(Expression(), $"column{values.Count + 1}"));
            } while (Match(","));
            Expect(")");
            rows.Add(new(null, values, null, [], null, 0));
        } while (Match(","));
        SelectRows Combine(int start, int count)
        {
            if (count == 1) return rows[start];
            int half = count / 2;
            return new(null, [], null, [], null, 0)
                { SetExpression = new(Combine(start, half), "UNION ALL", Combine(start + half, count - half)) };
        }
        return Combine(0, rows.Count);
    }

    private (string Table, string? Alias) QuerySource(Dictionary<string, SqlDerivedTable> derived)
    {
        if (!Match("(")) return (Name(), TableAlias());
        var query = QueryExpression();
        Expect(")");
        var alias = TableAlias() ?? throw Error("Give the derived table an alias.");
        var key = "\u0001derived" + ++derivedNumber;
        derived.Add(key, new(query, OptionalColumnList()));
        return (key, alias);
    }

    private SqlExpression ParseSubqueryAtom()
    {
        var kind = Match("EXISTS") ? SubqueryKind.Exists : SubqueryKind.Scalar;
        Expect("(");
        var inner = QueryExpression();
        Expect(")");
        return new SqlNestedQuery(inner, kind);
    }

    private SqlExpression MembershipSubquery(SqlExpression left)
    {
        var inner = QueryExpression();
        Expect(")");
        return new SqlNestedQuery(inner, SubqueryKind.In, left);
    }
}
