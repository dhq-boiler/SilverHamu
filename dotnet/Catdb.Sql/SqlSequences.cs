using System.Globalization;

namespace Catdb.Sql;

public sealed record CreateSequence(string Name, long Start, long Increment, bool IfNotExists) : SqlStatement;
public sealed record DropSequence(string Name, bool IfExists) : SqlStatement;
public sealed record AdvanceSequence(string Name, string Alias) : SqlStatement;
internal sealed record SqlNextValue(string Sequence) : SqlExpression
{
    public override SqlValue Evaluate(IReadOnlyDictionary<string, SqlValue> row) =>
        throw new SqlException("NEXT VALUE FOR requires a writable SQL sequence context.");
}

public sealed partial class SqlParser
{
    private bool StartsSequenceValue(int position) =>
        position + 2 < tokens.Count && IsWord(position, "NEXT") && IsWord(position + 1, "VALUE") && IsWord(position + 2, "FOR");

    private bool IsWord(int position, string text) => tokens[position].Kind == "word" &&
        tokens[position].Text.Equals(text, StringComparison.OrdinalIgnoreCase);

    private string NextSequenceName()
    {
        Expect("NEXT"); Expect("VALUE"); Expect("FOR");
        return Name();
    }

    private CreateSequence SequenceDefinition()
    {
        bool optional = Match("IF");
        if (optional) { Expect("NOT"); Expect("EXISTS"); }
        var name = Name();
        long? start = null, increment = null;
        while (Is("START") || Is("INCREMENT"))
        {
            if (Match("START"))
            {
                if (start.HasValue) throw Error("Duplicate sequence START.");
                Expect("WITH"); start = SequenceNumber();
            }
            else
            {
                if (increment.HasValue) throw Error("Duplicate sequence INCREMENT.");
                Expect("INCREMENT"); Expect("BY"); increment = SequenceNumber();
                if (increment == 0) throw Error("Sequence increment cannot be zero.");
            }
        }
        return new(name, start ?? 1, increment ?? 1, optional);
    }

    private long SequenceNumber()
    {
        var sign = Is("-") || Is("+") ? tokens[current++].Text : "";
        if (Peek.Kind != "number" || !long.TryParse(sign + Peek.Text, NumberStyles.AllowLeadingSign,
                CultureInfo.InvariantCulture, out var number))
            throw Error("A sequence requires a signed 64-bit integer.");
        current++;
        return number;
    }
}

public sealed partial class SqlDatabase
{
    private sealed record SequenceState(long? Next, long Increment);
    private static string SequenceKey(string name) => "_catdb_owner/sql/v1/sequence/" + TableId(name);

    private async Task<SqlResult> CreateSequenceAsync(CreateSequence sequence, List<(string Key, byte[]? Value)> changes)
    {
        var key = SequenceKey(sequence.Name);
        if (await GetAsync(key) is not null)
        {
            if (sequence.IfNotExists) return Empty($"Sequence '{sequence.Name}' already exists.");
            throw new SqlException($"Sequence '{sequence.Name}' already exists.");
        }
        changes.Add((key, Serialize(new SequenceState(sequence.Start, sequence.Increment))));
        return Empty($"Created sequence '{sequence.Name}'.");
    }

    private async Task<SqlResult> DropSequenceAsync(DropSequence sequence, List<(string Key, byte[]? Value)> changes)
    {
        var key = SequenceKey(sequence.Name);
        if (await GetAsync(key) is null)
        {
            if (sequence.IfExists) return Empty($"Sequence '{sequence.Name}' does not exist.");
            throw new SqlException($"Sequence '{sequence.Name}' does not exist.");
        }
        changes.Add((key, null));
        return Empty($"Dropped sequence '{sequence.Name}'.");
    }

    private async Task<SqlValue> TakeSequenceValueAsync(string name, Dictionary<string, SequenceState> pending)
    {
        var key = SequenceKey(name);
        if (!pending.TryGetValue(key, out var state))
            state = Deserialize<SequenceState>(await GetAsync(key) ?? throw new SqlException($"Unknown sequence '{name}'."));
        var value = state.Next ?? throw new SqlException($"Sequence '{name}' is exhausted.");
        var next = (Int128)value + state.Increment;
        pending[key] = state with { Next = next >= long.MinValue && next <= long.MaxValue ? (long)next : null };
        return SqlNumbers.Number(value);
    }

    private async Task<InsertValues> BindSequenceValuesAsync(InsertValues insert, List<(string Key, byte[]? Value)> changes)
    {
        var pending = new Dictionary<string, SequenceState>();
        var rows = new List<List<SqlExpression>>();
        foreach (var row in insert.Rows)
        {
            var bound = new List<SqlExpression>(row.Count);
            foreach (var expression in row)
                bound.Add(expression is SqlNextValue next
                    ? new SqlLiteral(await TakeSequenceValueAsync(next.Sequence, pending)) : expression);
            rows.Add(bound);
        }
        foreach (var (key, state) in pending) changes.Add((key, Serialize(state)));
        return insert with { Rows = rows };
    }
}
