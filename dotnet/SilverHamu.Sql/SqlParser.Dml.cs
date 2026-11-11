namespace SilverHamu.Sql;

internal sealed record ConflictAction(List<string>? Columns, Dictionary<string, SqlExpression>? Assignments, SqlExpression? Where);
internal sealed record TruncateTable(string Table) : SqlStatement;
public sealed record SqlTrigger(string Name, string Event, string Body, string? Condition);
internal sealed record CreateTrigger(string Table, SqlTrigger Trigger, bool IfNotExists) : SqlStatement;
internal sealed record DropTrigger(string Name, bool IfExists) : SqlStatement;

public sealed partial class SqlParser
{
    private bool inTriggerBody;

    private SqlStatement WriteTail(SqlStatement statement, bool allowConflict = false)
    {
        if (allowConflict && Match("ON"))
        {
            Expect("CONFLICT");
            var columns = OptionalColumnList();
            Expect("DO");
            Dictionary<string, SqlExpression>? assignments = null;
            SqlExpression? where = null;
            if (!Match("NOTHING"))
            {
                if (columns is null) throw Error("DO UPDATE requires a conflict target.");
                Expect("UPDATE"); Expect("SET");
                assignments = Assignments();
                if (Match("WHERE")) where = Expression();
            }
            statement = statement with { Conflict = new(columns, assignments, where) };
        }
        if (!Match("RETURNING")) return statement;
        var projections = new List<SqlProjection>();
        do
        {
            if (Match("*")) projections.Add(new(null, null, Star: true));
            else if (QualifiedStart && current + 2 < tokens.Count && tokens[current + 2] is { Kind: "symbol", Text: "*" })
            {
                var qualifier = Name(); Expect("."); Expect("*");
                projections.Add(new(null, null, Star: true, Qualifier: qualifier));
            }
            else
            {
                var expression = Expression();
                if (expression.ContainsAggregate() || SqlNestedQuery.Contains(expression)) throw Error("RETURNING requires scalar expressions over the affected row.");
                projections.Add(new(expression, Match("AS") ? Name() : (expression as SqlField)?.Name ?? "value"));
            }
            if (projections.Count > 64) throw Error("RETURNING supports up to 64 columns.");
        } while (Match(","));
        return statement with { Returning = projections };
    }

    private CreateTrigger TriggerDefinition()
    {
        bool optional = Match("IF");
        if (optional) { Expect("NOT"); Expect("EXISTS"); }
        var name = Name();
        Expect("AFTER");
        var operation = Match("INSERT") ? "INSERT" : Match("UPDATE") ? "UPDATE" : Match("DELETE") ? "DELETE"
            : throw Error("Specify AFTER INSERT, UPDATE or DELETE.");
        Expect("ON"); var table = Name();
        if (Match("FOR")) { Expect("EACH"); Expect("ROW"); }
        string? condition = null;
        if (Match("WHEN"))
        {
            int start = Peek.Position;
            if (SqlNestedQuery.Contains(Expression())) throw Error("A trigger WHEN cannot contain a subquery.");
            condition = source[start..Peek.Position];
        }
        Expect("BEGIN");
        int bodyStart = Peek.Position, commands = 0;
        inTriggerBody = true;
        while (!Is("END"))
        {
            if (Match(";")) continue;
            if (!Is("INSERT") && !Is("UPDATE") && !Is("DELETE")) throw Error("Trigger bodies support INSERT VALUES, UPDATE and DELETE.");
            conditionCount = 0;
            var command = Statement();
            if (command is not (InsertStatement or UpdateRows or DeleteRows) || command.Returning is not null || command.Conflict is not null)
                throw Error("Unsupported trigger action.");
            if (++commands > 32) throw Error("A trigger may contain at most 32 actions.");
            if (!Is("END")) Expect(";");
        }
        if (commands == 0) throw Error("A trigger needs an action.");
        string body = source[bodyStart..Peek.Position];
        Expect("END"); inTriggerBody = false;
        return new(table, new(name, operation, body, condition), optional);
    }
}
