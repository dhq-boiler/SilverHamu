using System.Text.Json.Nodes;

namespace SilverHamu.Sql;

public sealed partial class SqlDatabase
{
    private sealed record PendingTrigger(SqlTable Table, SqlTrigger Trigger, SqlRow? Old, SqlRow? New);
    private sealed record TriggerBody(IReadOnlyList<SqlStatement> Commands, SqlExpression? When);
    private sealed class TriggerRun
    {
        public Dictionary<SqlTrigger, TriggerBody> Programs { get; } = [];
        public int Depth;
        public int Actions;
    }

    private async Task<SqlResult> ChangeTriggerAsync(SqlStatement statement, List<(string Key, byte[]? Value)> changes)
    {
        var create = statement as CreateTrigger;
        var name = create?.Trigger.Name ?? ((DropTrigger)statement).Name;
        var tables = await GetTablesAsync();
        var owner = tables.FirstOrDefault(t => t.Triggers?.Any(trigger => SameTable(trigger.Name, name)) == true);
        if (create is null)
        {
            if (owner is null && !((DropTrigger)statement).IfExists) throw new SqlException($"Unknown trigger '{name}'.");
            if (owner is not null) changes.Add((TableKey(owner.Name), Serialize(owner with
                { Triggers = owner.Triggers!.Where(t => !SameTable(t.Name, name)).ToList() })));
            return Empty($"DROP TRIGGER {name}");
        }
        if (owner is not null)
        {
            if (create.IfNotExists) return Empty($"Trigger '{name}' already exists.");
            throw new SqlException($"Trigger '{name}' already exists.");
        }
        var table = tables.FirstOrDefault(t => SameTable(t.Name, create.Table)) ?? throw new SqlException("A trigger requires a base table.");
        if (table.Triggers is { Count: >= 32 }) throw new SqlException("A table may have up to 32 triggers.");
        await ValidateTriggerAsync(table, create.Trigger, tables);
        changes.Add((TableKey(table.Name), Serialize(table with { Triggers = [.. table.Triggers ?? [], create.Trigger] })));
        return Empty($"CREATE TRIGGER {name}");
    }

    private async Task ValidateTriggerAsync(SqlTable owner, SqlTrigger trigger, IReadOnlyList<SqlTable> tables)
    {
        var probe = new PendingTrigger(owner, trigger, null, null);
        var body = await ReadTriggerBodyAsync(trigger);
        if (body.When is not null) ValidatePredicate(BindTriggerExpression(body.When, probe), []);
        foreach (var raw in body.Commands)
        {
            var command = BindTriggerCommand(raw, probe);
            var target = tables.FirstOrDefault(t => SameTable(t.Name, TriggerTarget(command)))
                ?? throw new SqlException("Trigger actions require existing base tables.");
            switch (command)
            {
                case InsertStatement insert:
                    var columns = InsertColumns(target, insert.Columns);
                    var values = insert is InsertValues i ? i.Rows : ((InsertRows)insert).Rows
                        .Select(row => row.Select(v => (SqlExpression)new SqlLiteral(v)).ToList()).ToList();
                    foreach (var row in values)
                    {
                        if (columns.Count != row.Count) throw new SqlException("Trigger INSERT column count mismatch.");
                        for (int c = 0; c < row.Count; c++) ValidateAssignment(columns[c], row[c], []);
                    }
                    break;
                case UpdateRows update:
                    ResolveAssignments(target, update.Assignments);
                    ValidatePredicate(update.Where, target.Columns);
                    break;
                case DeleteRows delete:
                    ValidatePredicate(delete.Where, target.Columns);
                    break;
            }
        }
    }

    private static string TriggerTarget(SqlStatement command) => command switch
    {
        InsertStatement insert => insert.Table, UpdateRows update => update.Table, DeleteRows delete => delete.Table,
        _ => throw new SqlException("Unsupported trigger action.")
    };

    private async Task RestrictTriggerTargetDropAsync(SqlTable target)
    {
        foreach (var table in await GetTablesAsync())
        {
            if (SameTable(table.Name, target.Name) || table.Triggers is null) continue;
            foreach (var trigger in table.Triggers)
                if (SqlParser.Parse(trigger.Body, triggerBody: true).Any(command => SameTable(TriggerTarget(command), target.Name)))
                    throw new SqlException($"Trigger '{trigger.Name}' references table '{target.Name}'.");
        }
    }

    private async Task ValidateAlteredTriggersAsync(SqlTable changed)
    {
        var tables = (await GetTablesAsync()).Select(t => SameTable(t.Name, changed.Name) ? changed : t).ToList();
        foreach (var owner in tables)
            foreach (var trigger in owner.Triggers ?? [])
                if (SameTable(owner.Name, changed.Name) || SqlParser.Parse(trigger.Body, triggerBody: true)
                    .Any(command => SameTable(TriggerTarget(command), changed.Name)))
                    await ValidateTriggerAsync(owner, trigger, tables);
    }

    private async Task<TriggerBody> ReadTriggerBodyAsync(SqlTrigger trigger)
    {
        var commands = SqlParser.Parse(trigger.Body, triggerBody: true);
        foreach (var command in commands)
        {
            _ = TriggerTarget(command);
            if (command.Conflict is not null || command.Returning is not null) throw new SqlException("Invalid stored trigger action.");
            await BindExpressionsAsync(command.ExpressionBindings);
        }
        SqlExpression? when = null;
        if (trigger.Condition is { } condition)
        {
            when = SqlParser.ParseCheck(condition, out var bindings);
            await BindExpressionsAsync(bindings);
        }
        return new(commands, when);
    }

    private static SqlExpression BindTriggerExpression(SqlExpression expression, PendingTrigger pending) =>
        expression.Rewrite(node =>
        {
            if (node is SqlAggregate or SqlNestedQuery) throw new SqlException("Trigger actions require scalar expressions.");
            if (node is not SqlField { Qualifier: { } qualifier } field) return null;
            bool old = SameTable(qualifier, "OLD");
            if (!old && !SameTable(qualifier, "NEW")) return null;
            if (old && pending.Trigger.Event == "INSERT" || !old && pending.Trigger.Event == "DELETE")
                throw new SqlException($"{qualifier} is not available for {pending.Trigger.Event}.");
            var column = Column(pending.Table, field.Name);
            return new SqlBoundQuery(column.Type, _ => (old ? pending.Old : pending.New)?.Values[column.Name] ?? SqlValue.Null);
        });

    private static SqlStatement BindTriggerCommand(SqlStatement command, PendingTrigger pending)
    {
        string target = TriggerTarget(command);
        SqlExpression Bind(SqlExpression expression) => BindTriggerExpression(expression, pending).Rewrite(node =>
            node is SqlField { Qualifier: { } qualifier } field && SameTable(qualifier, target)
                ? new SqlField(field.Name) : null);
        return command switch
        {
            InsertValues insert => insert with { Rows = insert.Rows.Select(r => r.Select(Bind).ToList()).ToList() },
            InsertRows insert => insert,
            UpdateRows update => UpdateRows.FromAssignments(update.Table,
                update.Assignments.ToDictionary(p => p.Key, p => Bind(p.Value), StringComparer.OrdinalIgnoreCase),
                update.Where is null ? null : Bind(update.Where)),
            DeleteRows delete => delete with { Where = delete.Where is null ? null : Bind(delete.Where) },
            _ => throw new SqlException("Unsupported trigger action.")
        };
    }

    private static void QueueTriggers(List<PendingTrigger> queue, SqlTable table, SqlRow? old, SqlRow? next)
    {
        if (table.Triggers is not { Count: > 0 }) return;
        var operation = old is null ? "INSERT" : next is null ? "DELETE" : "UPDATE";
        foreach (var trigger in table.Triggers)
            if (trigger.Event == operation)
            {
                if (queue.Count >= MaxRows * 4) throw new SqlException("Trigger event limit exceeded.");
                queue.Add(new(table, trigger, old, next));
            }
    }

    private async Task FireTriggersAsync(List<PendingTrigger> queue, TriggerRun run)
    {
        if (run.Depth >= 16) throw new SqlException("Trigger nesting exceeds 16 levels.");
        run.Depth++;
        try
        {
            foreach (var pending in queue)
            {
                if (!run.Programs.TryGetValue(pending.Trigger, out var body))
                    run.Programs.Add(pending.Trigger, body = await ReadTriggerBodyAsync(pending.Trigger));
                if (body.When is not null && !BindTriggerExpression(body.When, pending).Evaluate(new Dictionary<string, SqlValue>()).IsTrue)
                    continue;
                foreach (var command in body.Commands)
                {
                    if (++run.Actions > MaxRows) throw new SqlException("Statement exceeds 4096 trigger actions.");
                    await ExecuteStatementAsync(BindTriggerCommand(command, pending), triggerRun: run);
                }
            }
        }
        finally { run.Depth--; }
    }

    private Task<JsonObject> TriggerSavepointAsync(string operation, string name) =>
        send(new JsonObject { ["op"] = operation, ["name"] = name });
}
