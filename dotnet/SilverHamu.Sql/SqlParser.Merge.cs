namespace SilverHamu.Sql;

internal sealed record MergeSource(string? Table, SelectRows? Query, List<List<SqlExpression>>? Values, string Alias, List<string>? Columns);
internal abstract record MergeAction;
internal sealed record MergeUpdate(Dictionary<string, SqlExpression> Assignments) : MergeAction;
internal sealed record MergeDelete : MergeAction;
internal sealed record MergeInsert(List<string>? Columns, List<SqlExpression> Values) : MergeAction;
internal sealed record MergeWhen(bool Matched, SqlExpression? Condition, MergeAction Action);
internal sealed record MergeRows(string Target, string Alias, MergeSource Source, SqlExpression On, List<MergeWhen> Clauses) : SqlStatement;

public sealed partial class SqlParser
{
    private Dictionary<string, SqlExpression> Assignments()
    {
        var assignments = new Dictionary<string, SqlExpression>(StringComparer.OrdinalIgnoreCase);
        do
        {
            var column = Name();
            Expect("=");
            if (!assignments.TryAdd(column, Expression())) throw Error($"列 '{column}' が重複しています。");
        } while (Match(","));
        return assignments;
    }

    private bool HasColumnListBeforeAs() => Is("(") && current + 2 < tokens.Count &&
        tokens[current + 1].Kind is "identifier" or "word" && tokens[current + 2] is { Kind: "symbol", Text: "," or ")" };

    private List<string>? OptionalColumnList()
    {
        if (!Match("(")) return null;
        var columns = new List<string>();
        do
        {
            var name = Name();
            if (columns.Contains(name, StringComparer.OrdinalIgnoreCase)) throw Error($"列 '{name}' が重複しています。");
            columns.Add(name);
            if (columns.Count > 64) throw Error("列数は64以下にしてください。");
        } while (Match(","));
        Expect(")");
        return columns;
    }

    private List<SqlExpression> MergeValueList()
    {
        Expect("(");
        var values = new List<SqlExpression>();
        do
        {
            values.Add(Expression());
            if (values.Count > 64) throw Error("列数は64以下にしてください。");
        } while (Match(","));
        Expect(")");
        return values;
    }

    private MergeRows Merge()
    {
        Expect("INTO");
        var target = Name();
        var targetAlias = MergeAlias("USING") ?? target;
        Expect("USING");
        string? sourceTable = null;
        SelectRows? query = null;
        List<List<SqlExpression>>? values = null;
        if (Match("("))
        {
            if (Match("VALUES"))
            {
                values = [];
                do
                {
                    values.Add(MergeValueList());
                    if (values.Count > SqlDatabase.MaxRows) throw Error("VALUES の行数が上限を超えました。");
                } while (Match(","));
            }
            else query = QueryExpression();
            Expect(")");
        }
        else sourceTable = Name();
        var alias = MergeAlias("ON") ?? sourceTable ?? throw Error("MERGE の入力に別名を指定してください。");
        var source = new MergeSource(sourceTable, query, values, alias, OptionalColumnList());
        Expect("ON");
        var on = Expression();
        var clauses = new List<MergeWhen>();
        do
        {
            Expect("WHEN");
            var matched = !Match("NOT");
            Expect("MATCHED");
            if (clauses.Any(c => c.Matched == matched && c.Condition is null))
                throw Error("無条件の WHEN の後に同じ種類の WHEN は指定できません。");
            var condition = Match("AND") ? Expression() : null;
            Expect("THEN");
            MergeAction action;
            if (!matched)
            {
                Expect("INSERT");
                var columns = OptionalColumnList();
                Expect("VALUES");
                action = new MergeInsert(columns, MergeValueList());
            }
            else if (Match("DELETE")) action = new MergeDelete();
            else { Expect("UPDATE"); Expect("SET"); action = new MergeUpdate(Assignments()); }
            clauses.Add(new(matched, condition, action));
        } while (Is("WHEN"));
        return new(target, targetAlias, source, on, clauses);
    }

    private string? MergeAlias(string nextKeyword)
    {
        if (Match("AS")) return Name();
        return !Is(nextKeyword) && Peek.Kind is ("word" or "identifier") ? Name() : null;
    }
}
