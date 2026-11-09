using System.Text;

namespace Catdb.Sql;

public sealed class SqlParser
{
    private sealed record Token(string Text, string Kind, int Position);
    private readonly List<Token> tokens;
    private int current;
    private int conditionCount;
    private bool inProcedureBody;
    private readonly string source;
    private SqlParser(string sql) { source = sql; tokens = Lex(sql); }
    public static IReadOnlyList<SqlStatement> Parse(string sql)
    {
        if (sql.Length > 1024 * 1024) throw new SqlException("SQLは1 MiB以下にしてください。");
        var parser = new SqlParser(sql);
        var statements = new List<SqlStatement>();
        while (!parser.End)
        {
            if (parser.Match(";")) continue;
            parser.conditionCount = 0;
            statements.Add(parser.Statement());
            if (!parser.End) parser.Expect(";");
            if (statements.Count > 100) throw new SqlException("1回の実行は100文までです。");
        }
        if (statements.Count == 0) throw new SqlException("SQL文を入力してください。");
        return statements;
    }

    private SqlStatement Statement()
    {
        if (Match("CREATE")) return Create();
        if (Match("EXEC") || Match("EXECUTE")) return new ExecuteProcedure(Name());
        if (Is("GRANT") || Is("REVOKE"))
        {
            var grant = Match("GRANT");
            if (!grant) Expect("REVOKE");
            Expect("SELECT"); Expect("ON");
            var view = Name();
            Expect(grant ? "TO" : "FROM");
            return new ViewPrivilege(view, Name(), grant);
        }
        if (Match("ALTER"))
        {
            Expect("TABLE"); var table = Name();
            if (Match("ADD")) { Match("COLUMN"); return new AddColumn(table, ColumnDefinition()); }
            Expect("RENAME"); Expect("COLUMN"); var column = Name(); Expect("TO");
            return new RenameColumn(table, column, Name());
        }
        if (Match("REINDEX")) return new ReindexTable(Name());
        if (Match("DROP"))
        {
            var kind = SchemaObjectKind();
            var optional = Match("IF"); if (optional) Expect("EXISTS");
            var name = Name();
            if (kind == "PROCEDURE") return new DropProcedure(name, optional);
            if (kind == "INDEX")
            {
                Expect("ON");
                return new DropIndex(name, Name(), optional);
            }
            return kind == "VIEW" ? new DropView(name, optional) : new DropTable(name, optional);
        }
        if (Match("INSERT")) return Insert();
        if (Match("SELECT")) return Select();
        if (Match("UPDATE"))
        {
            var table = Name(); Expect("SET"); var values = new Dictionary<string, SqlValue>(StringComparer.OrdinalIgnoreCase);
            do { var name = Name(); Expect("="); if (!values.TryAdd(name, Literal())) throw Error("同じ列が複数回指定されています。"); } while (Match(","));
            return new UpdateRows(table, values, Match("WHERE") ? Expression() : null);
        }
        if (Match("DELETE")) { Expect("FROM"); var table = Name(); return new DeleteRows(table, Match("WHERE") ? Expression() : null); }
        if (Match("BEGIN")) { Match("TRANSACTION"); return new TransactionStatement("begin"); }
        if (Match("COMMIT")) { Match("TRANSACTION"); return new TransactionStatement("commit"); }
        if (Match("ROLLBACK")) { Match("TRANSACTION"); return new TransactionStatement("rollback"); }
        if (Match("SHOW")) { if (Match("VIEWS")) return new ShowViews(); Expect("TABLES"); return new ShowTables(); }
        throw Error("未対応のSQL文です。");
    }

    private string SchemaObjectKind()
    {
        if (Match("TABLE")) return "TABLE";
        if (Match("VIEW")) return "VIEW";
        if (Match("INDEX")) return "INDEX";
        if (Match("PROCEDURE")) return "PROCEDURE";
        throw Error("TABLE、VIEW、INDEX、PROCEDUREのいずれかを指定してください。");
    }

    private SqlStatement Create()
    {
        var kind = SchemaObjectKind();
        var optional = Match("IF"); if (optional) { Expect("NOT"); Expect("EXISTS"); }
        var objectName = Name();
        if (kind == "PROCEDURE") return ProcedureDefinition(objectName, optional);
        if (kind == "INDEX")
        {
            Expect("ON");
            var table = Name();
            Expect("(");
            var indexColumns = new List<string>();
            do
            {
                var column = Name();
                if (indexColumns.Contains(column, StringComparer.OrdinalIgnoreCase)) throw Error("索引の列が重複しています。");
                indexColumns.Add(column);
                if (indexColumns.Count > 64) throw Error("索引は64列までです。");
            } while (Match(","));
            Expect(")");
            return new CreateIndex(objectName, table, indexColumns, optional);
        }
        if (kind == "VIEW")
        {
            Expect("AS");
            var start = Peek.Position;
            Expect("SELECT");
            var select = Select(allowSubquery: false);
            if (select.Table is null) throw Error("VIEWのFROMには元テーブルを指定してください。");
            return new CreateView(objectName, select, source[start..(End ? source.Length : Peek.Position)], optional);
        }
        Expect("("); var columns = new List<SqlColumn>();
        var checks = new List<string>();
        var references = new Dictionary<string, SqlReference>(StringComparer.OrdinalIgnoreCase);
        List<string>? primaryColumns = null;
        do
        {
            if (Is("PRIMARY") && current + 1 < tokens.Count && tokens[current + 1].Text.Equals("KEY", StringComparison.OrdinalIgnoreCase))
            {
                if (primaryColumns is not null) throw Error("主キーは1つだけ指定してください。");
                Expect("PRIMARY"); Expect("KEY"); Expect("(");
                primaryColumns = [];
                do
                {
                    var key = Name();
                    if (primaryColumns.Contains(key, StringComparer.OrdinalIgnoreCase)) throw Error("主キーの列が重複しています。");
                    primaryColumns.Add(key);
                    if (primaryColumns.Count > 64) throw Error("主キーは64列までです。");
                } while (Match(","));
                Expect(")");
                continue;
            }
            if (Is("CHECK") && current + 1 < tokens.Count && tokens[current + 1] is { Kind: "symbol", Text: "(" })
            {
                Expect("CHECK"); checks.Add(CheckDefinition()); continue;
            }
            if (Is("FOREIGN") && current + 1 < tokens.Count && tokens[current + 1].Text.Equals("KEY", StringComparison.OrdinalIgnoreCase))
            {
                Expect("FOREIGN"); Expect("KEY"); Expect("("); var child = Name(); Expect(")"); Expect("REFERENCES");
                if (!references.TryAdd(child, ReferenceDefinition())) throw Error("同じ列の外部キーが重複しています。");
                continue;
            }
            var column = ColumnDefinition();
            if (columns.Any(x => x.Name.Equals(column.Name, StringComparison.OrdinalIgnoreCase))) throw Error("同じ列名が複数回指定されています。");
            columns.Add(column);
            if (columns.Count > 64) throw Error("列数は64以下にしてください。");
        } while (Match(","));
        Expect(")");
        if (columns.Count == 0) throw Error("テーブルには1列以上必要です。");
        foreach (var (child, reference) in references)
        {
            var pos = columns.FindIndex(c => c.Name.Equals(child, StringComparison.OrdinalIgnoreCase));
            if (pos < 0) throw Error($"外部キーの列 '{child}' は存在しません。");
            if (columns[pos].Reference is not null) throw Error("同じ列の外部キーが重複しています。");
            columns[pos] = columns[pos] with { Reference = reference };
        }
        if (columns.Count(x => x.PrimaryKey) + (primaryColumns is null ? 0 : 1) > 1) throw Error("主キーは1つだけ指定してください。");
        if (primaryColumns is not null)
        {
            for (var i = 0; i < primaryColumns.Count; i++)
            {
                var pos = columns.FindIndex(c => c.Name.Equals(primaryColumns[i], StringComparison.OrdinalIgnoreCase));
                if (pos < 0) throw Error($"主キーの列 '{primaryColumns[i]}' が見つかりません。");
                var column = columns[pos];
                columns[pos] = column with { NotNull = true, PrimaryKey = primaryColumns.Count == 1, Unique = column.Unique || primaryColumns.Count == 1 };
                primaryColumns[i] = column.Name;
            }
        }
        return new CreateTable(objectName, columns, optional, checks.Count == 0 ? null : checks, primaryColumns is { Count: > 1 } ? primaryColumns : null);
    }

    private SqlReference ReferenceDefinition()
    {
        var table = Name(); Expect("("); var column = Name(); Expect(")");
        return new(table, column);
    }

    private string CheckDefinition()
    {
        Expect("(");
        var start = Peek.Position;
        Expression();
        var end = Peek.Position;
        Expect(")");
        return source[start..end];
    }

    internal static SqlExpression ParseCheck(string text)
    {
        var parser = new SqlParser(text);
        var expression = parser.Expression();
        if (!parser.End) throw parser.Error("CHECK式が不正です。");
        return expression;
    }

    private SqlColumn ColumnDefinition()
    {
        var name = Name(); var type = Name().ToUpperInvariant();
        type = type switch { "INT" or "BIGINT" => "INTEGER", "VARCHAR" or "NVARCHAR" or "STRING" => "TEXT", "FLOAT" or "DOUBLE" or "DECIMAL" => "REAL", "BOOL" => "BOOLEAN", "VARBINARY" => "BLOB", _ => type };
        if (type is not ("INTEGER" or "REAL" or "TEXT" or "BOOLEAN" or "BLOB")) throw Error($"型 '{type}' は未対応です。INTEGER / REAL / TEXT / BOOLEAN / BLOBを使用してください。");
        int? maxLength = null;
        if (Match("(")) { if (type is not ("TEXT" or "BLOB")) throw Error("長さ指定はTEXT / BLOBのみ対応します。"); maxLength = Integer(1, 65536); Expect(")"); }
        bool notNull = false, unique = false, primary = false; SqlValue? defaultValue = null;
        SqlReference? reference = null;
        var checks = new List<string>();
        while (true)
        {
            if (Match("PRIMARY")) { Expect("KEY"); primary = true; notNull = true; unique = true; }
            else if (Match("NOT")) { Expect("NULL"); notNull = true; }
            else if (Match("NULL")) { if (notNull) throw Error("NULL制約が矛盾しています。"); }
            else if (Match("UNIQUE")) unique = true;
            else if (Match("DEFAULT")) defaultValue = Literal();
            else if (Match("REFERENCES"))
            {
                if (reference is not null) throw Error("同じ列の外部キーが重複しています。");
                reference = ReferenceDefinition();
            }
            else if (Match("CHECK")) checks.Add(CheckDefinition());
            else break;
        }
        return new(name, type, notNull, unique, primary, defaultValue, maxLength, reference, checks.Count == 0 ? null : checks);
    }

    private InsertRows Insert()
    {
        Expect("INTO"); var table = Name(); List<string>? columns = null;
        if (Match("(")) { columns = []; do { columns.Add(Name()); } while (Match(",")); Expect(")"); if (columns.Distinct(StringComparer.OrdinalIgnoreCase).Count() != columns.Count) throw Error("INSERTの列名が重複しています。"); }
        Expect("VALUES"); var rows = new List<List<SqlValue>>();
        do { Expect("("); var values = new List<SqlValue>(); do { values.Add(Literal()); } while (Match(",")); Expect(")"); rows.Add(values); if (rows.Count > 4096) throw Error("1文の挿入は4096行までです。"); } while (Match(","));
        return new(table, columns, rows);
    }

    private SelectRows Select(bool allowSubquery = true)
    {
        int? limit = null;
        if (Match("TOP")) { var parentheses = Match("("); limit = Integer(0, 4096); if (parentheses) Expect(")"); }
        var projections = new List<SqlProjection>();
        do
        {
            if (Match("*")) projections.Add(new(null, null, Star: true));
            else if (QualifiedStart && current + 2 < tokens.Count && tokens[current + 2] is { Kind: "symbol", Text: "*" })
            {
                var qualifier = Name(); Expect("."); Expect("*");
                projections.Add(new(null, null, Star: true, Qualifier: qualifier));
            }
            else if (Is("COUNT") && current + 1 < tokens.Count && tokens[current + 1] is { Kind: "symbol", Text: "(" })
            {
                Expect("COUNT"); Expect("("); Expect("*"); Expect(")");
                projections.Add(new(null, Match("AS") ? Name() : "count", Count: true));
            }
            else if ((Is("SUM") || Is("AVG")) && current + 1 < tokens.Count && tokens[current + 1] is { Kind: "symbol", Text: "(" })
            {
                var function = Name().ToUpperInvariant();
                Expect("("); var column = Field(); Expect(")");
                projections.Add(new(column, Match("AS") ? Name() : function.ToLowerInvariant(),
                    Aggregate: function == "SUM" ? NumericAggregate.Sum : NumericAggregate.Average));
            }
            else { var expression = Atom(); projections.Add(new(expression, Match("AS") ? Name() : expression is SqlField field ? field.Name : "value")); }
            if (projections.Count > 64) throw Error("結果列は64列以下にしてください。");
        } while (Match(","));
        string? table = null, alias = null;
        SqlJoin? join = null;
        List<SqlJoin>? additionalJoins = null;
        if (Match("FROM"))
        {
            table = Name(); alias = TableAlias();
            while (Is("JOIN") || Is("INNER") || Is("LEFT") || Is("RIGHT") || Is("FULL"))
            {
                var kind = SqlJoinKind.Inner;
                if (Match("LEFT")) kind = SqlJoinKind.Left;
                else if (Match("RIGHT")) kind = SqlJoinKind.Right;
                else if (Match("FULL")) kind = SqlJoinKind.Full;
                else Match("INNER");
                if (kind != SqlJoinKind.Inner) Match("OUTER");
                Expect("JOIN");
                var other = Name(); var otherAlias = TableAlias();
                Expect("ON");
                var left = Field(); Expect("="); var right = Field();
                if (left.Qualifier is null || right.Qualifier is null)
                    throw Error("ONの両辺をテーブル名または別名で修飾してください。");
                var next = new SqlJoin(other, otherAlias, left, right, kind);
                if (join is null) join = next;
                else
                {
                    additionalJoins ??= [];
                    if (additionalJoins.Count >= 6) throw Error("JOINで参照できる表は8個までです。");
                    additionalJoins.Add(next);
                }
            }
        }
        var where = Match("WHERE") ? Expression() : null;
        if (Match("IN"))
        {
            if (!allowSubquery || where is not SqlField field) throw Error("IN 副問い合わせは SELECT の WHERE 列名に指定してください。");
            Expect("("); Expect("SELECT");
            var inner = Select(allowSubquery: false);
            Expect(")");
            if (inner.Table is null || inner.Join is not null || inner.IsGrouped ||
                inner.Projections is not [{ Expression: SqlField }])
                throw Error("IN の副問い合わせには1つの表またはビューから1列を指定してください。");
            where = new SqlInSubquery(field, inner);
        }
        List<SqlField>? groupBy = null;
        if (Match("GROUP"))
        {
            Expect("BY"); groupBy = [];
            do
            {
                groupBy.Add(Field());
                if (groupBy.Count > 64) throw Error("GROUP BYは64列以下にしてください。");
            } while (Match(","));
        }
        var order = new List<SqlOrder>();
        if (Match("ORDER")) { Expect("BY"); do { var field = Field(); var descending = Match("DESC"); if (!descending) Match("ASC"); order.Add(new(field.Name, descending, field.Qualifier)); } while (Match(",")); }
        if (Match("LIMIT")) { if (limit is not null) throw Error("TOPとLIMITは併用できません。"); limit = Integer(0, 4096); }
        var offset = Match("OFFSET") ? Integer(0, 4096) : 0;
        return new(table, projections, where, order, limit, offset, alias, join, groupBy, additionalJoins);
    }

    private string? TableAlias()
    {
        if (inProcedureBody && Is("END")) return null;
        if (Match("AS")) return Name();
        if (Peek.Kind == "identifier") return Name();
        if (Peek.Kind != "word") return null;
        return Peek.Text.ToUpperInvariant() switch
        {
            "INNER" or "JOIN" or "ON" or "WHERE" or "GROUP" or "ORDER" or "LIMIT" or "OFFSET" or
            "LEFT" or "RIGHT" or "FULL" or "OUTER" or "NATURAL" or "CROSS" => null,
            _ => Name()
        };
    }

    private CreateProcedure ProcedureDefinition(string name, bool optional)
    {
        Expect("AS"); Expect("BEGIN");
        var start = Peek.Position;
        var count = 0;
        inProcedureBody = true;
        while (!Is("END"))
        {
            if (Match(";")) continue;
            if (End) throw Error("プロシージャーのENDが必要です。");
            if (!(Is("SELECT") || Is("INSERT") || Is("UPDATE") || Is("DELETE")))
                throw Error("プロシージャー内に指定できるのはSELECT・INSERT・UPDATE・DELETEです。");
            conditionCount = 0;
            Statement();
            if (++count > 100) throw Error("1つのプロシージャーは100文までです。");
            if (!Is("END")) Expect(";");
        }
        if (count == 0) throw Error("プロシージャーにSQL文を指定してください。");
        var body = source[start..Peek.Position];
        Expect("END");
        inProcedureBody = false;
        return new(name, body, optional);
    }

    private bool QualifiedStart => Peek.Kind is "word" or "identifier" && current + 1 < tokens.Count && tokens[current + 1] is { Kind: "symbol", Text: "." };
    private SqlField Field()
    {
        var first = Name();
        return Match(".") ? new SqlField(Name(), first) : new SqlField(first);
    }

    private SqlExpression Expression(int depth = 0)
    {
        if (depth > 32) throw Error("条件式の括弧が深すぎます。");
        var expression = And(depth);
        while (Match("OR")) expression = new SqlBinary(expression, "OR", And(depth));
        return expression;
    }
    private SqlExpression And(int depth)
    {
        var expression = Condition(depth);
        while (Match("AND")) expression = new SqlBinary(expression, "AND", Condition(depth));
        return expression;
    }
    private SqlExpression Condition(int depth)
    {
        if (++conditionCount > 128) throw Error("条件式は128個以下にしてください。");
        if (Match("(")) { var expression = Expression(depth + 1); Expect(")"); return expression; }
        var left = Atom();
        if (Match("IS")) { var negated = Match("NOT"); Expect("NULL"); return new SqlIsNull(left, negated); }
        foreach (var op in new[] { "=", "<>", "!=", "<=", ">=", "<", ">", "LIKE" })
            if (Match(op)) return new SqlBinary(left, op, Atom());
        return left;
    }
    private SqlExpression Atom()
    {
        if (QualifiedStart) return Field();
        return Peek.Kind is "string" or "number" || Is("NULL") || Is("TRUE") || Is("FALSE") || (Is("X") || Is("N")) && current + 1 < tokens.Count && tokens[current + 1].Kind == "string"
            ? new SqlLiteral(Literal()) : Field();
    }
    private SqlValue Literal()
    {
        if (Match("NULL")) return SqlValue.Null;
        if (Match("TRUE")) return SqlValue.Boolean(true);
        if (Match("FALSE")) return SqlValue.Boolean(false);
        if (Match("X"))
        {
            if (Peek.Kind != "string") throw Error("バイナリーはX'0123AB'形式で指定してください。");
            try { return new("BLOB", Convert.ToHexString(Convert.FromHexString(tokens[current++].Text))); }
            catch (FormatException) { throw Error("バイナリーの16進文字列が不正です。"); }
        }
        Match("N");
        if (Peek.Kind == "string") return SqlValue.String(tokens[current++].Text);
        if (Peek.Kind == "number") return SqlValue.Number(tokens[current++].Text);
        throw Error("値には文字列・数値・TRUE/FALSE・NULL・X'hex'を指定してください。");
    }
    private int Integer(int min, int max)
    {
        if (Peek.Kind != "number" || !int.TryParse(tokens[current++].Text, out var value) || value < min || value > max) throw Error($"整数{min}～{max}を指定してください。");
        return value;
    }
    private string Name()
    {
        if (Peek.Kind is not ("word" or "identifier")) throw Error("テーブル名または列名を指定してください。");
        var name = tokens[current++].Text;
        if (name.Length is < 1 or > 128 || name.Any(char.IsControl)) throw Error("名前は制御文字を含まない1～128文字にしてください。");
        return name;
    }
    private bool End => current == tokens.Count;
    private Token Peek => End ? new("", "end", tokens.Count == 0 ? 0 : tokens[^1].Position + tokens[^1].Text.Length) : tokens[current];
    private bool Is(string word) => Peek.Kind is not ("string" or "identifier") && Peek.Text.Equals(word, StringComparison.OrdinalIgnoreCase);
    private bool Match(string word) { if (!Is(word)) return false; current++; return true; }
    private void Expect(string word) { if (!Match(word)) throw Error($"'{word}' が必要です。"); }
    private SqlException Error(string message) => new($"SQL位置 {Peek.Position + 1}: {message}");

    private static List<Token> Lex(string sql)
    {
        var tokens = new List<Token>(); var i = 0;
        while (i < sql.Length)
        {
            var start = i; var c = sql[i];
            if (char.IsWhiteSpace(c)) { i++; continue; }
            if (c == '-' && i + 1 < sql.Length && sql[i + 1] == '-') { while (i < sql.Length && sql[i] != '\n') i++; continue; }
            if (c == '/' && i + 1 < sql.Length && sql[i + 1] == '*') { var end = sql.IndexOf("*/", i + 2, StringComparison.Ordinal); if (end < 0) throw new SqlException($"SQL位置 {i + 1}: コメントが閉じられていません。"); i = end + 2; continue; }
            if (c is '\'' or '"' or '[')
            {
                var close = c == '[' ? ']' : c; var value = new StringBuilder(); i++; var closed = false;
                while (i < sql.Length)
                {
                    if (sql[i] == close) { i++; if (i < sql.Length && sql[i] == close) { value.Append(close); i++; } else { closed = true; break; } }
                    else value.Append(sql[i++]);
                }
                if (!closed) throw new SqlException($"SQL位置 {start + 1}: 引用符が閉じられていません。");
                tokens.Add(new(value.ToString(), c == '\'' ? "string" : "identifier", start));
            }
            else if (char.IsLetter(c) || c == '_') { i++; while (i < sql.Length && (char.IsLetterOrDigit(sql[i]) || sql[i] == '_')) i++; tokens.Add(new(sql[start..i], "word", start)); }
            else if (char.IsDigit(c) || c is '+' or '-') { i++; while (i < sql.Length && (char.IsDigit(sql[i]) || sql[i] is '.' or 'e' or 'E' || sql[i] is '+' or '-' && sql[i - 1] is 'e' or 'E')) i++; tokens.Add(new(sql[start..i], "number", start)); }
            else if ("(),;.*=<>!".Contains(c)) { i++; if (i < sql.Length && (c is '<' or '>' or '!' && sql[i] == '=' || c == '<' && sql[i] == '>')) i++; tokens.Add(new(sql[start..i], "symbol", start)); }
            else throw new SqlException($"SQL位置 {i + 1}: 未対応の文字 '{c}' です。");
            if (tokens.Count > 50000) throw new SqlException("SQLの要素数が上限を超えています。");
        }
        return tokens;
    }
}
