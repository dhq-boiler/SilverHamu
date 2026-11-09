using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlDatabase
{
    private sealed class RowSet(SqlTable table, IEnumerable<SqlRow> original, IEnumerable<SqlRow> current)
    {
        public SqlTable Table { get; } = table;
        public Dictionary<string, SqlRow> Original { get; } = original.ToDictionary(r => r.StorageKey);
        public Dictionary<string, SqlRow> Current { get; } = current.ToDictionary(r => r.StorageKey);
    }

    private sealed record ForeignKeyChange(RowSet State, SqlRow Original, SqlRow Before, SqlRow? After);

    // Plan every affected table before writing. Cascade edges follow row identity, so two
    // parent rows can exchange key values without moving a child twice or losing its index.
    private async Task ApplyReferentialActionsAsync(SqlTable table, List<SqlRow> original, List<SqlRow> proposed,
        List<(string Key, byte[]? Value)> changes, List<PendingTrigger>? triggers = null)
    {
        var root = new RowSet(table, original, proposed);
        var initialAssignments = proposed.ToDictionary(r => r.StorageKey);
        var states = new Dictionary<string, RowSet>(StringComparer.OrdinalIgnoreCase) { [table.Name] = root };
        var incoming = (await GetTablesAsync()).SelectMany(child => ForeignKeys(child).Select(fk => (Child: child, Key: fk)))
            .ToLookup(link => link.Key.Table, StringComparer.OrdinalIgnoreCase);
        var queue = new Queue<ForeignKeyChange>();
        foreach (var row in original)
        {
            root.Current.TryGetValue(row.StorageKey, out var after);
            if (!ReferenceEquals(row, after)) queue.Enqueue(new(root, row, row, after));
        }
        var dependents = new Dictionary<SqlForeignKey, ILookup<string?, SqlRow>>();
        var assignments = new Dictionary<(string Row, string Column), SqlValue>();
        while (queue.TryDequeue(out var change))
        {
            foreach (var (child, fk) in incoming[change.State.Table.Name])
            {
                var before = ReferenceIdentity(change.Before, fk.Targets);
                var after = change.After is null ? null : ReferenceIdentity(change.After, fk.Targets);
                if (before == after) continue;
                if (!states.TryGetValue(child.Name, out var children))
                {
                    var rows = await LoadRowsAsync(child);
                    states.Add(child.Name, children = new(child, rows, rows));
                }
                // Even a restricted child is in states so final-state validation covers it.
                var action = change.After is null
                    ? fk.OnDelete ?? (fk.DeleteCascade ? SqlReferenceAction.Cascade : SqlReferenceAction.Restrict)
                    : fk.OnUpdate ?? (fk.UpdateCascade ? SqlReferenceAction.Cascade : SqlReferenceAction.Restrict);
                if (action == SqlReferenceAction.Restrict) continue;
                if (!dependents.TryGetValue(fk, out var byParent))
                {
                    byParent = children.Original.Values.Where(r => children.Current.ContainsKey(r.StorageKey) &&
                        (!ReferenceEquals(children, root) || !initialAssignments.TryGetValue(r.StorageKey, out var initial) ||
                         ReferenceIdentity(r, fk.Columns) == ReferenceIdentity(initial, fk.Columns)))
                        .ToLookup(r => ReferenceIdentity(r, fk.Columns), StringComparer.Ordinal);
                    dependents.Add(fk, byParent);
                }
                var originalKey = ReferenceIdentity(change.Original, fk.Targets);
                if (originalKey is null) continue;
                foreach (var oldChild in byParent[originalKey])
                {
                    if (!children.Current.TryGetValue(oldChild.StorageKey, out var current)) continue;
                    SqlRow? next = null;
                    if (change.After is not null || action != SqlReferenceAction.Cascade)
                    {
                        Dictionary<string, SqlValue>? replacement = null;
                        for (int i = 0; i < fk.Columns.Count; i++)
                        {
                            var field = Column(child, fk.Columns[i]);
                            var column = field.Name;
                            var value = action switch
                            {
                                SqlReferenceAction.SetNull => SqlValue.Null,
                                SqlReferenceAction.SetDefault => field.Default ?? SqlValue.Null,
                                _ => change.After!.Values[fk.Targets[i]]
                            };
                            if (action == SqlReferenceAction.Cascade && SqlValue.Compare(change.Before.Values[fk.Targets[i]], value) == 0) continue;
                            var slot = (current.StorageKey, column);
                            if (assignments.TryGetValue(slot, out var previous) && SqlValue.Compare(previous, value) != 0)
                                throw new SqlException("複数のCASCADEが同じ列に異なる値を指定しています。");
                            assignments[slot] = value;
                            if (SqlValue.Compare(current.Values[column], value) == 0) continue;
                            replacement ??= new(current.Values, StringComparer.OrdinalIgnoreCase);
                            replacement[column] = value;
                        }
                        if (replacement is null) continue;
                        next = new(current.StorageKey, replacement);
                        children.Current[current.StorageKey] = next;
                    }
                    else children.Current.Remove(current.StorageKey);
                    queue.Enqueue(new(children, oldChild, current, next));
                }
            }
        }

        foreach (var state in states.Values)
        {
            var rows = state.Current.Values.ToList();
            foreach (var row in rows) ValidateRow(state.Table, row.Values);
            ValidateUnique(state.Table, rows);
            await ValidateChecksAsync(state.Table, rows);
            await ValidateOutgoingReferencesAsync(state.Table, rows, states);
            RecordRowChanges(state, changes);
            if (triggers is not null && state.Table.Triggers is { Count: > 0 })
                foreach (var key in state.Original.Keys.Union(state.Current.Keys))
                {
                    state.Original.TryGetValue(key, out var old);
                    state.Current.TryGetValue(key, out var next);
                    if (!ReferenceEquals(old, next)) QueueTriggers(triggers, state.Table, old, next);
                }
        }
    }

    private static void RecordRowChanges(RowSet state, List<(string Key, byte[]? Value)> changes)
    {
        var table = state.Table;
        var primary = table.PrimaryIndex ? table.Columns.Single(c => c.PrimaryKey) : null;
        var indexes = table.Indexes?.Select(i => new IndexLayout(table, i)).ToArray() ?? [];
        var secondary = new Dictionary<string, byte[]?>(StringComparer.Ordinal);
        var primaryDeletes = new HashSet<string>(StringComparer.Ordinal);
        var primaryPuts = new Dictionary<string, byte[]>(StringComparer.Ordinal);
        foreach (var key in state.Original.Keys.Union(state.Current.Keys))
        {
            state.Original.TryGetValue(key, out var before);
            state.Current.TryGetValue(key, out var after);
            if (ReferenceEquals(before, after)) continue;
            changes.Add((key, after is null ? null : Serialize(after.Values)));
            foreach (var index in indexes) index.RecordChange(key, before?.Values, after?.Values, secondary);
            if (primary is null) continue;
            var oldKey = before is null ? null : IndexKey(table, before.Values[primary.Name]);
            var newKey = after is null ? null : IndexKey(table, after.Values[primary.Name]);
            if (newKey == oldKey) continue;
            if (oldKey is not null) primaryDeletes.Add(oldKey);
            if (newKey is not null) primaryPuts[newKey] = Encoding.UTF8.GetBytes(key);
        }
        foreach (var key in primaryDeletes.Except(primaryPuts.Keys)) changes.Add((key, null));
        foreach (var (key, value) in primaryPuts) changes.Add((key, value));
        foreach (var (key, value) in secondary) changes.Add((key, value));
    }
}
