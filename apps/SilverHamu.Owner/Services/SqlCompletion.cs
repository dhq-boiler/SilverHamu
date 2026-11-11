using System.Text;
using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed record SqlCompletionItem(string Name, string Kind, string InsertText)
{
    public string Label => $"{Name}  · {Kind}";
}

internal sealed record SqlCompletionSet(int Start, int Length, IReadOnlyList<SqlCompletionItem> Items)
{
    public (string Text, int Cursor) Apply(string text, SqlCompletionItem item) =>
        (text[..Start] + item.InsertText + text[(Start + Length)..], Start + item.InsertText.Length);
}

internal static class SqlCompletion
{
    private static readonly string[] Keywords = ("SELECT FROM WHERE JOIN LEFT RIGHT FULL INNER OUTER CROSS ON AS " +
        "AND OR NOT IS NULL IN EXISTS BETWEEN LIKE DISTINCT GROUP BY HAVING ORDER ASC DESC " +
        "OFFSET FETCH FIRST NEXT ROW ROWS ONLY UNION ALL WITH CASE WHEN THEN ELSE END " +
        "COUNT SUM AVG MIN MAX CAST COALESCE CREATE TABLE VIEW INSERT INTO VALUES UPDATE SET DELETE " +
        "BEGIN COMMIT ROLLBACK SHOW TABLES VIEWS ANALYZE EXPLAIN").Split(' ');

    private sealed record Token(string Text, int Start, int End, bool Identifier = true, bool Protected = false, bool Quoted = false);

    private sealed record WriteContext(string? Table, bool Tables = false, bool Columns = false,
        string[]? Words = null, HashSet<string>? UsedColumns = null);

    public static SqlCompletionSet Suggest(string text, int cursor, IEnumerable<SqlTable> schema, bool force = false)
    {
        cursor = Math.Clamp(cursor, 0, text.Length);
        var tokens = Tokenize(text);
        var active = tokens.FirstOrDefault(t => t.Start <= cursor && cursor <= t.End &&
            (t.Identifier || t.Protected));
        if (active is { Protected: true }) return new(cursor, 0, []);
        int start = active?.Start ?? cursor, end = active?.End ?? cursor;
        var prefix = text[start..cursor];
        if (prefix.StartsWith('[')) prefix = prefix[1..].Replace("]]", "]");
        else if (prefix.StartsWith('"')) prefix = prefix[1..].Replace("\"\"", "\"");
        var preceding = tokens.Where(t => t.End <= start).ToList();
        int lastStatement = preceding.FindLastIndex(t => t.Text == ";");
        int statementStart = lastStatement < 0 ? 0 : preceding[lastStatement].End;
        preceding = preceding.Skip(lastStatement + 1).ToList();
        string previous = preceding.LastOrDefault()?.Text.ToUpperInvariant() ?? "";
        bool tablePosition = previous is "FROM" or "JOIN" or "UPDATE" or "INTO";
        bool qualified = previous == ".";
        bool expressionPosition = previous is "WHERE" or "AND" or "OR" or "ON" or "HAVING";
        var tables = schema.ToArray();
        var statement = tokens.SkipWhile(t => t.End <= statementStart)
            .TakeWhile(t => t.Text != ";").ToArray();
        var write = GetWriteContext(preceding);
        if (!force && prefix.Length == 0 && !tablePosition && !qualified && !expressionPosition && write is null)
            return new(start, end - start, []);
        if (write is not null)
        {
            var candidates = new List<SqlCompletionItem>();
            if (write.Tables) candidates.AddRange(tables.Select(t => new SqlCompletionItem(t.Name,
                t.ViewQuery is null ? L.Get("Table") : L.Get("View"), SqlIdentifiers.Format(t.Name))));
            if (write.Columns) candidates.AddRange(tables.Where(t => t.Name.Equals(write.Table, StringComparison.OrdinalIgnoreCase))
                .SelectMany(t => t.Columns).Where(c => write.UsedColumns?.Contains(c.Name) != true)
                .Select(c => new SqlCompletionItem(c.Name, L.Get("Column"), SqlIdentifiers.Format(c.Name))));
            candidates.AddRange((write.Words ?? []).Select(w => new SqlCompletionItem(w, "SQL", w)));
            return Finish(start, end - start, prefix, candidates, force);
        }
        if (statement.FirstOrDefault()?.Text.Equals("INSERT", StringComparison.OrdinalIgnoreCase) == true)
        {
            int select = Array.FindIndex(statement, t => Word(t, "SELECT") || Word(t, "WITH"));
            if (select >= 0 && statement[select].Start < start) statement = statement[select..];
        }
        var sources = new Dictionary<string, SqlTable>(StringComparer.OrdinalIgnoreCase);
        var aliases = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        for (int i = 0; i + 1 < statement.Length; i++)
        {
            if (statement[i].Text.ToUpperInvariant() is not ("FROM" or "JOIN" or "UPDATE" or "INTO")) continue;
            var table = tables.FirstOrDefault(t => t.Name.Equals(statement[i + 1].Text, StringComparison.OrdinalIgnoreCase));
            if (table is not null) sources[table.Name] = table;
            int alias = i + 2;
            if (alias < statement.Length && statement[alias].Text.Equals("AS", StringComparison.OrdinalIgnoreCase)) alias++;
            if (alias >= statement.Length || !statement[alias].Identifier) continue;
            var token = statement[alias];
            bool quoted = text[token.Start] is '[' or '"';
            if (!quoted && (Keywords.Contains(token.Text, StringComparer.OrdinalIgnoreCase) ||
                token.Text.ToUpperInvariant() is "LIMIT" or "TOP" or "USING" or "NATURAL")) continue;
            aliases[token.Text] = quoted ? SqlIdentifiers.Quote(token.Text) : token.Text;
            if (table is not null) sources[token.Text] = table;
        }
        IEnumerable<SqlTable> columnSources = sources.Count == 0 ? tables : sources.Values.Distinct();
        if (qualified)
        {
            string qualifier = preceding.Count >= 2 ? preceding[^2].Text : "";
            columnSources = sources.TryGetValue(qualifier, out var table) ? [table] :
                tables.Where(t => t.Name.Equals(qualifier, StringComparison.OrdinalIgnoreCase));
        }
        var items = new List<SqlCompletionItem>();
        if (!qualified)
        {
            items.AddRange(tables.Select(t => new SqlCompletionItem(t.Name, t.ViewQuery is null ? L.Get("Table") : L.Get("View"), SqlIdentifiers.Format(t.Name))));
            if (!tablePosition) items.AddRange(Keywords.Select(k => new SqlCompletionItem(k, "SQL", k)));
            if (!tablePosition) items.AddRange(aliases.Select(a => new SqlCompletionItem(a.Key, L.Get("Variable"), a.Value)));
        }
        if (!tablePosition) items.AddRange(columnSources.SelectMany(t => t.Columns)
            .Select(c => new SqlCompletionItem(c.Name, L.Get("Column"), SqlIdentifiers.Format(c.Name))));
        return Finish(start, end - start, prefix, items, force);
    }

    private static SqlCompletionSet Finish(int start, int length, string prefix,
        IEnumerable<SqlCompletionItem> items, bool force) => new(start, length, items
            .Where(i => i.Name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            .Where(i => force || !i.Name.Equals(prefix, StringComparison.OrdinalIgnoreCase))
            .DistinctBy(i => (i.Kind, i.Name.ToUpperInvariant()))
            .OrderBy(i => i.Kind == L.Get("Variable") ? -1 : i.Kind == "SQL" ? (prefix.Length == 0 ? 0 : 1) : (prefix.Length == 0 ? 1 : 0))
            .ThenBy(i => i.Kind == "SQL" ? Array.IndexOf(Keywords, i.Name) : 0)
            .ThenBy(i => i.Name, StringComparer.OrdinalIgnoreCase)
            .Take(40).ToArray());

    private static bool Word(Token token, string word) => token.Identifier && !token.Quoted &&
        token.Text.Equals(word, StringComparison.OrdinalIgnoreCase);

    private static WriteContext? GetWriteContext(List<Token> input)
    {
        var tokens = input.Where(t => !t.Protected || t.Quoted || !(t.Text.StartsWith("--") || t.Text.StartsWith("/*"))).ToArray();
        if (tokens.Length == 0) return null;
        bool update = Word(tokens[0], "UPDATE"), delete = Word(tokens[0], "DELETE"), insert = Word(tokens[0], "INSERT");
        if (!update && !delete && !insert) return null;
        if (tokens.Length == 1)
            return update ? new(null, Tables: true) : new(null, Words: [delete ? "FROM" : "INTO"]);
        int tableIndex = update ? 1 : 2;
        if (!update && !Word(tokens[1], delete ? "FROM" : "INTO")) return new(null);
        if (tokens.Length <= tableIndex) return new(null, Tables: true);
        string table = tokens[tableIndex].Text;
        var tail = tokens[(tableIndex + 1)..];
        var conditions = new[] { "NULL", "TRUE", "FALSE", "NOT", "AND", "OR", "IS", "IN", "BETWEEN", "LIKE", "EXISTS", "CAST", "COALESCE" };
        if (insert)
        {
            int source = Array.FindIndex(tail, t => Word(t, "SELECT") || Word(t, "WITH"));
            if (source >= 0) return null;
            int values = Array.FindIndex(tail, t => Word(t, "VALUES"));
            if (values >= 0)
            {
                int depth = 0;
                foreach (var token in tail[(values + 1)..])
                {
                    if (token.Text == "(" && !token.Identifier && !token.Protected) depth++;
                    if (token.Text == ")" && !token.Identifier && !token.Protected) depth--;
                }
                var last = tail[^1];
                if (depth == 0) return new(table, Words: last.Text == ")" ? [","] : ["("]);
                if (Word(last, "CAST")) return new(table, Words: ["("]);
                if (Word(last, "AS")) return new(table, Words: ["TEXT", "INTEGER", "REAL", "BLOB", "BOOLEAN"]);
                if (!last.Identifier && !last.Protected && last.Text is "(" or ",")
                    return new(table, Words: ["NULL", "TRUE", "FALSE", "CAST", "NEXT VALUE FOR"]);
                if (depth > 1) return new(table, Words: last.Identifier &&
                    new[] { "TEXT", "INTEGER", "REAL", "BLOB", "BOOLEAN" }.Contains(last.Text.ToUpperInvariant()) ? [")"] : ["AS"]);
                return new(table, Words: [",", ")"]);
            }
            if (tail.Length == 0) return new(table, Words: ["(", "VALUES", "SELECT", "WITH"]);
            if (tail[0].Text != "(") return new(table);
            if (tail.Any(t => t.Text == ")" && !t.Identifier && !t.Protected))
                return new(table, Words: ["VALUES", "SELECT", "WITH"]);
            var used = tail.Skip(1).Where(t => t.Identifier).Select(t => t.Text).ToHashSet(StringComparer.OrdinalIgnoreCase);
            return tail[^1].Text is "(" or "," ? new(table, Columns: true, UsedColumns: used) : new(table, Words: [",", ")"]);
        }
        if (tail.Any(t => Word(t, "WHERE"))) return new(table, Columns: true, Words: conditions);
        if (delete) return new(table, Words: ["WHERE"]);
        if (tail.Length == 0) return new(table, Words: ["SET"]);
        if (!Word(tail[0], "SET")) return new(table);
        int assignment = 1, nesting = 0;
        var assigned = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        for (int i = 1; i < tail.Length; i++)
        {
            var token = tail[i];
            if (token.Identifier || token.Protected) continue;
            if (token.Text == "(") nesting++;
            else if (token.Text == ")") nesting--;
            else if (token.Text == "," && nesting == 0)
            {
                if (tail[assignment].Identifier) assigned.Add(tail[assignment].Text);
                assignment = i + 1;
            }
        }
        if (assignment == tail.Length) return new(table, Columns: true, UsedColumns: assigned);
        if (assignment + 1 == tail.Length) return new(table, Words: ["="]);
        return new(table, Columns: true, Words: assignment + 2 == tail.Length ? conditions : [.. conditions, "WHERE"]);
    }

    private static List<Token> Tokenize(string text)
    {
        var tokens = new List<Token>();
        for (int i = 0; i < text.Length;)
        {
            if (char.IsWhiteSpace(text[i])) { i++; continue; }
            int start = i;
            if (i + 1 < text.Length && text[i..(i + 2)] is "--" or "/*")
            {
                bool line = text[i + 1] == '-'; i += 2;
                while (i < text.Length && (line ? text[i] is not ('\n' or '\r') : !(i + 1 < text.Length && text[i] == '*' && text[i + 1] == '/'))) i++;
                if (!line && i < text.Length) i += 2;
                tokens.Add(new(text[start..i], start, i, false, true)); continue;
            }
            if (text[i] is '\'' or '"' or '[')
            {
                char open = text[i++], close = open == '[' ? ']' : open;
                var value = new StringBuilder();
                while (i < text.Length)
                {
                    if (text[i] == close)
                    {
                        i++;
                        if (i == text.Length || text[i] != close) break;
                    }
                    value.Append(text[i++]);
                }
                tokens.Add(new(value.ToString(), start, i, open != '\'', open == '\'', true)); continue;
            }
            if (char.IsLetter(text[i]) || text[i] is '_' or '$')
            {
                while (i < text.Length && (char.IsLetterOrDigit(text[i]) || text[i] is '_' or '$')) i++;
                tokens.Add(new(text[start..i], start, i)); continue;
            }
            i++;
            tokens.Add(new(text[start..i], start, i, false));
        }
        return tokens;
    }
}
