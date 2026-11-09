using System.Text;

namespace Catdb.Sql;

public sealed partial class SqlParser
{
    private sealed record Token(string Text, string Kind, int Position);
    private readonly List<Token> tokens;
    private int current;
    private int conditionCount;
    private bool inProcedureBody;
    private readonly string source;
    private SqlParser(string sql) { source = sql; tokens = Lex(sql); }
    public static IReadOnlyList<SqlStatement> Parse(string sql) => Parse(sql, triggerBody: false);

    internal static IReadOnlyList<SqlStatement> Parse(string sql, bool triggerBody)
    {
        if (sql.Length > 1024 * 1024) throw new SqlException("SQLは1 MiB以下にしてください。");
        var parser = new SqlParser(sql) { inTriggerBody = triggerBody };
        var statements = new List<SqlStatement>();
        while (!parser.End)
        {
            if (parser.Match(";")) continue;
            parser.conditionCount = 0;
            parser.compoundCount = 0;
            var statement = parser.Statement();
            if (parser.bindings.NamedCasts.Count > 0 || parser.bindings.Additions.Count > 0)
            {
                statement = statement with { ExpressionBindings = parser.bindings };
                parser.bindings = new();
            }
            statements.Add(statement);
            if (!parser.End) parser.Expect(";");
            if (statements.Count > 100) throw new SqlException("1回の実行は100文までです。");
        }
        if (statements.Count == 0) throw new SqlException("SQL文を入力してください。");
        return statements;
    }

    private SqlStatement Statement()
    {
        if (Match("EXPLAIN"))
        {
            bool analyze = Match("ANALYZE");
            if (!(Is("SELECT") || Is("WITH"))) throw Error("EXPLAIN requires a SELECT or WITH query.");
            return new ExplainQuery(QueryExpression(), analyze);
        }
        if (Match("CREATE")) return Create();
        if (Match("TRUNCATE")) { Expect("TABLE"); return new TruncateTable(Name()); }
        if (Match("ANALYZE")) return new AnalyzeTable(Name());
        if (Match("EXEC") || Match("EXECUTE")) return new ExecuteProcedure(Name());
        if (Is("GRANT") || Is("REVOKE"))
        {
            var grant = Match("GRANT");
            if (!grant) Expect("REVOKE");
            ViewAccess access = 0;
            do
            {
                if (Match("SELECT")) access |= ViewAccess.Select;
                else if (Match("INSERT")) access |= ViewAccess.Insert;
                else if (Match("UPDATE")) access |= ViewAccess.Update;
                else if (Match("DELETE")) access |= ViewAccess.Delete;
                else throw Error("SELECT、INSERT、UPDATE、DELETE のいずれかを指定してください。");
            } while (Match(","));
            Expect("ON");
            var view = Name();
            Expect(grant ? "TO" : "FROM");
            return new ViewPrivilege(view, Name(), grant) { Access = access };
        }
        if (Match("ALTER"))
        {
            if (Match("TYPE"))
            {
                var type = Name();
                Expect("ENABLE");
                Expect("OPERATOR");
                Expect("+");
                return new EnableAddition(type);
            }
            Expect("TABLE"); var table = Name();
            if (Match("ADD")) { Match("COLUMN"); return new AddColumn(table, ColumnDefinition()); }
            if (Match("DROP")) { Match("COLUMN"); return new DropColumn(table, Name()); }
            if (Match("ALTER")) return ColumnAlteration(table);
            Expect("RENAME"); Expect("COLUMN"); var column = Name(); Expect("TO");
            return new RenameColumn(table, column, Name());
        }
        if (Match("REINDEX")) return new ReindexTable(Name());
        if (Match("DROP"))
        {
            if (Match("SEQUENCE"))
            {
                bool ifExists = Match("IF");
                if (ifExists) Expect("EXISTS");
                return new DropSequence(Name(), ifExists);
            }
            if (Match("TRIGGER"))
            {
                var optionalTrigger = Match("IF"); if (optionalTrigger) Expect("EXISTS");
                return new DropTrigger(Name(), optionalTrigger);
            }
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
        if (Match("INSERT")) return WriteTail(Insert(), allowConflict: true);
        if (Match("MERGE")) return Merge();
        if (Is("SELECT") && StartsSequenceValue(current + 1))
        {
            Expect("SELECT");
            var sequence = NextSequenceName();
            return new AdvanceSequence(sequence, Match("AS") ? Name() : "value");
        }
        if (Is("SELECT") || Is("WITH"))
        {
            var query = QueryExpression();
            if (!Match("FOR")) return query;
            Expect("UPDATE");
            return new LockRows(query);
        }
        if (Match("UPDATE"))
        {
            var table = Name(); Expect("SET"); var values = Assignments();
            return WriteTail(UpdateRows.FromAssignments(table, values, Match("WHERE") ? Expression() : null));
        }
        if (Match("DELETE")) { Expect("FROM"); var table = Name(); return WriteTail(new DeleteRows(table, Match("WHERE") ? Expression() : null)); }
        if (Match("BEGIN")) { Match("TRANSACTION"); return new TransactionStatement("begin"); }
        if (Match("START")) { Expect("TRANSACTION"); return new TransactionStatement("begin"); }
        if (Match("SAVEPOINT")) return new TransactionStatement("savepoint", Name());
        if (Match("RELEASE")) { Match("SAVEPOINT"); return new TransactionStatement("release_savepoint", Name()); }
        if (Match("COMMIT")) { Match("TRANSACTION"); return new TransactionStatement("commit"); }
        if (Match("ROLLBACK"))
        {
            Match("TRANSACTION");
            if (!Match("TO")) return new TransactionStatement("rollback");
            Match("SAVEPOINT"); return new TransactionStatement("rollback_to", Name());
        }
        if (Match("SHOW"))
        {
            if (Match("VIEWS")) return new ShowViews();
            if (Match("TYPES")) return new ShowTypes();
            Expect("TABLES"); return new ShowTables();
        }
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
        if (Match("TRIGGER")) return TriggerDefinition();
        if (Match("TYPE")) return TypeDefinition();
        if (Match("SEQUENCE")) return SequenceDefinition();
        bool uniqueIndex = Match("UNIQUE");
        if (uniqueIndex && !Is("INDEX")) throw Error("Expected INDEX after UNIQUE.");
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
            string? predicate = null;
            if (Match("WHERE"))
            {
                int start = Peek.Position;
                Expression();
                predicate = source[start..Peek.Position].TrimEnd();
            }
            return new CreateIndex(objectName, table, indexColumns, optional) { Unique = uniqueIndex, Predicate = predicate };
        }
        if (kind == "VIEW")
        {
            Expect("AS");
            var start = Peek.Position;
            var select = QueryExpression();
            if (select.HasQuerySources) throw Error("A published view definition must use base tables directly.");
            if (select.Table is null) throw Error("VIEWのFROMには元テーブルを指定してください。");
            return new CreateView(objectName, select, source[start..(End ? source.Length : Peek.Position)], optional);
        }
        if (Match("AS")) return new CreateTableAs(objectName, null, QueryExpression(), optional);
        if (HasColumnListBeforeAs())
        {
            var names = OptionalColumnList(); Expect("AS");
            return new CreateTableAs(objectName, names, QueryExpression(), optional);
        }
        Expect("("); var columns = new List<SqlColumn>();
        var checks = new List<string>();
        var references = new List<SqlForeignKey>();
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
                Expect("FOREIGN"); Expect("KEY"); var children = ForeignKeyColumns(); Expect("REFERENCES");
                references.Add(ReferenceDefinition(children));
                continue;
            }
            var column = ColumnDefinition();
            if (columns.Any(x => x.Name.Equals(column.Name, StringComparison.OrdinalIgnoreCase))) throw Error("同じ列名が複数回指定されています。");
            columns.Add(column);
            if (columns.Count > 64) throw Error("列数は64以下にしてください。");
        } while (Match(","));
        Expect(")");
        if (columns.Count == 0) throw Error("テーブルには1列以上必要です。");
        foreach (var key in references.Where(fk => fk.Columns.Count == 1))
        {
            var pos = columns.FindIndex(c => c.Name.Equals(key.Columns[0], StringComparison.OrdinalIgnoreCase));
            if (pos < 0) throw Error($"外部キーの列 '{key.Columns[0]}' は存在しません。");
            if (columns[pos].Reference is not null) throw Error("同じ列の外部キーが重複しています。");
            columns[pos] = columns[pos] with { Reference = key.SingleReference() };
        }
        references.RemoveAll(key => key.Columns.Count == 1);
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
        return new CreateTable(objectName, columns, optional, checks.Count == 0 ? null : checks,
            primaryColumns is { Count: > 1 } ? primaryColumns : null, references.Count == 0 ? null : references);
    }

    private List<string> ForeignKeyColumns()
    {
        Expect("(");
        var names = new List<string>();
        do
        {
            var name = Name();
            if (names.Contains(name, StringComparer.OrdinalIgnoreCase) || names.Count == 64)
                throw Error("外部キーには重複のない64列以下を指定してください。");
            names.Add(name);
        } while (Match(","));
        Expect(")");
        return names;
    }

    private SqlForeignKey ReferenceDefinition(List<string> children)
    {
        var table = Name();
        var targets = ForeignKeyColumns();
        if (children.Count != targets.Count) throw Error("外部キーの列数が参照先と一致しません。");
        SqlReferenceAction? delete = null, update = null;
        while (Match("ON"))
        {
            var isDelete = Match("DELETE");
            if (!isDelete) Expect("UPDATE");
            if ((isDelete ? delete : update).HasValue) throw Error("外部キーの参照アクションが重複しています。");
            SqlReferenceAction action;
            if (Match("CASCADE")) action = SqlReferenceAction.Cascade;
            else if (Match("SET"))
            {
                if (Match("NULL")) action = SqlReferenceAction.SetNull;
                else { Expect("DEFAULT"); action = SqlReferenceAction.SetDefault; }
            }
            else
            {
                if (!Match("RESTRICT")) { Expect("NO"); Expect("ACTION"); }
                action = SqlReferenceAction.Restrict;
            }
            if (isDelete) delete = action; else update = action;
        }
        return new(children, table, targets, delete == SqlReferenceAction.Cascade, update == SqlReferenceAction.Cascade)
        {
            OnDelete = delete is SqlReferenceAction.SetNull or SqlReferenceAction.SetDefault ? delete : null,
            OnUpdate = update is SqlReferenceAction.SetNull or SqlReferenceAction.SetDefault ? update : null
        };
    }

    private string CheckDefinition()
    {
        Expect("(");
        var start = Peek.Position;
        if (SqlNestedQuery.Contains(Expression())) throw Error("CHECK cannot read another relation.");
        var end = Peek.Position;
        Expect(")");
        return source[start..end];
    }

    internal static SqlExpression ParseCheck(string text) => ParseCheck(text, out _);

    internal static SqlExpression ParseCheck(string text, out SqlExpressionBindings bindings)
    {
        var parser = new SqlParser(text);
        var expression = parser.Expression();
        if (SqlNestedQuery.Contains(expression)) throw parser.Error("CHECK cannot read another relation.");
        if (!parser.End) throw parser.Error("CHECK式が不正です。");
        bindings = parser.bindings;
        return expression;
    }

    private (string Type, int? MaxLength) ColumnType()
    {
        var name = Name();
        if (name.Equals("DECIMAL", StringComparison.OrdinalIgnoreCase) && Match("("))
        {
            var precision = Integer(1, 28);
            Expect(",");
            var scale = Integer(0, precision);
            Expect(")");
            return (FormattableString.Invariant($"DECIMAL({precision},{scale})"), null);
        }
        var type = BuiltInName(name) ?? name;
        int? maxLength = null;
        if (Match("(")) { if (type is not ("TEXT" or "BLOB")) throw Error("長さ指定はTEXT / BLOBのみ対応します。"); maxLength = Integer(1, 65536); Expect(")"); }
        return (type, maxLength);
    }

    private static string? BuiltInName(string name)
    {
        var upper = name.ToUpperInvariant();
        return upper switch
        {
            "INT" or "BIGINT" => "INTEGER",
            "VARCHAR" or "NVARCHAR" or "STRING" => "TEXT",
            "FLOAT" or "DOUBLE" or "DECIMAL" => "REAL",
            "BOOL" => "BOOLEAN",
            "VARBINARY" => "BLOB",
            _ => SqlTypeRules.IsBuiltIn(upper) ? upper : null
        };
    }

    private CreateType TypeDefinition()
    {
        var optional = Match("IF");
        if (optional) { Expect("NOT"); Expect("EXISTS"); }
        var name = Name();
        if (BuiltInName(name) is not null || name.ToUpperInvariant() is "NULL" or "NUMBER")
            throw Error($"'{name}' is reserved and cannot name a distinct type.");
        Expect("AS");
        Expect("DISTINCT");
        var (baseType, maxLength) = ColumnType();
        if (!SqlTypeRules.IsBuiltIn(baseType))
            throw Error("A distinct type requires a built-in base type.");
        var check = Match("CHECK") ? CheckDefinition() : null;
        return new(new(name, baseType, maxLength, check), optional);
    }

    private AlterColumn ColumnAlteration(string table)
    {
        Match("COLUMN");
        var name = Name();
        if (Match("SET"))
        {
            if (Match("DEFAULT")) return new(table, name, SqlColumnAlteration.SetDefault, Default: Literal());
            if (Match("NOT")) { Expect("NULL"); return new(table, name, SqlColumnAlteration.SetNotNull); }
            Expect("DATA"); Expect("TYPE");
        }
        else if (Match("DROP"))
        {
            if (Match("DEFAULT")) return new(table, name, SqlColumnAlteration.DropDefault);
            Expect("NOT"); Expect("NULL"); return new(table, name, SqlColumnAlteration.DropNotNull);
        }
        else Match("TYPE");
        var (type, maxLength) = ColumnType();
        return new(table, name, SqlColumnAlteration.Type, type, maxLength);
    }

    private SqlColumn ColumnDefinition()
    {
        var name = Name();
        var (type, maxLength) = ColumnType();
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
                var fk = ReferenceDefinition([name]);
                reference = fk.SingleReference();
            }
            else if (Match("CHECK")) checks.Add(CheckDefinition());
            else break;
        }
        return new(name, type, notNull, unique, primary, defaultValue, maxLength, reference, checks.Count == 0 ? null : checks);
    }

    private SqlStatement Insert()
    {
        Expect("INTO"); var table = Name(); var columns = OptionalColumnList();
        if (Is("SELECT") || Is("WITH")) return new InsertSelect(table, columns, QueryExpression());
        Expect("VALUES");
        var rows = new List<List<SqlExpression>>();
        bool hasCast = false;
        do
        {
            Expect("(");
            var values = new List<SqlExpression>();
            do
            {
                if (StartsSequenceValue(current))
                {
                    if (inTriggerBody) throw Error("NEXT VALUE FOR is not supported inside a trigger.");
                    values.Add(new SqlNextValue(NextSequenceName()));
                    hasCast = true;
                }
                else if (inTriggerBody)
                {
                    values.Add(Expression());
                    hasCast = true;
                }
                else if (Is("CAST"))
                {
                    var expression = Factor(0);
                    var operand = expression;
                    while (operand is SqlCast cast) operand = cast.Operand;
                    if (operand is not SqlLiteral) throw Error("INSERT VALUES の CAST にはリテラルを指定してください。");
                    values.Add(expression);
                    hasCast = true;
                }
                else values.Add(new SqlLiteral(Literal()));
            } while (Match(","));
            Expect(")");
            rows.Add(values);
            if (rows.Count > 4096) throw Error("1文の挿入は4096行までです。");
        } while (Match(","));
        return hasCast ? new InsertValues(table, columns, rows)
            : new InsertRows(table, columns, rows.Select(row => row.Cast<SqlLiteral>().Select(l => l.Value).ToList()).ToList());
    }

    private SelectRows SelectCore()
    {
        var distinct = Match("DISTINCT");
        int? limit = null;
        if (Match("TOP")) { var parentheses = Match("("); limit = Integer(0, 4096); if (parentheses) Expect(")"); }
        var projections = new List<SqlProjection>();
        do
        {
            if (Match("*")) projections.Add(new(null, null, Star: true));
            else if (WindowProjectionStart()) projections.Add(ParseWindowProjection());
            else if (QualifiedStart && current + 2 < tokens.Count && tokens[current + 2] is { Kind: "symbol", Text: "*" })
            {
                var qualifier = Name(); Expect("."); Expect("*");
                projections.Add(new(null, null, Star: true, Qualifier: qualifier));
            }
            else
            {
                var expression = Expression();
                var name = Match("AS") ? Name() : expression switch
                {
                    SqlField field => field.Name,
                    SqlAggregate aggregate => aggregate.Function.ToLowerInvariant(),
                    _ => "value"
                };
                projections.Add(expression is SqlAggregate { Function: "COUNT", Argument: null, Filter: null }
                    ? new(null, name, Count: true) : new(expression, name));
            }
            if (projections.Count > 64) throw Error("結果列は64列以下にしてください。");
        } while (Match(","));
        var derived = new Dictionary<string, SqlDerivedTable>(StringComparer.OrdinalIgnoreCase);
        string? table = null, alias = null;
        SqlJoin? join = null;
        List<SqlJoin>? additionalJoins = null;
        if (Match("FROM"))
        {
            (table, alias) = QuerySource(derived);
            while (Is("JOIN") || Is("INNER") || Is("LEFT") || Is("RIGHT") || Is("FULL") || Is("CROSS") || Is("NATURAL"))
            {
                var natural = Match("NATURAL");
                var kind = SqlJoinKind.Inner;
                if (!natural && Match("CROSS")) kind = SqlJoinKind.Cross;
                else if (Match("LEFT")) kind = SqlJoinKind.Left;
                else if (Match("RIGHT")) kind = SqlJoinKind.Right;
                else if (Match("FULL")) kind = SqlJoinKind.Full;
                else Match("INNER");
                if (kind is SqlJoinKind.Left or SqlJoinKind.Right or SqlJoinKind.Full) Match("OUTER");
                Expect("JOIN");
                var (other, otherAlias) = QuerySource(derived);
                var next = new SqlJoin(other, otherAlias, null, null, kind) { Natural = natural };
                if (!natural && kind != SqlJoinKind.Cross)
                {
                    if (Match("USING"))
                    {
                        Expect("(");
                        var keys = new List<string>();
                        do { keys.Add(Name()); } while (Match(","));
                        Expect(")");
                        if (keys.Distinct(StringComparer.OrdinalIgnoreCase).Count() != keys.Count)
                            throw Error("Duplicate column in JOIN USING.");
                        next = next with { Using = keys };
                    }
                    else
                    {
                        Expect("ON");
                        var condition = Expression();
                        next = condition is SqlBinary { Operator: "=", Left: SqlField left, Right: SqlField right }
                            ? next with { Left = left, Right = right, On = condition }
                            : next with { On = condition };
                    }
                }
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
        List<SqlExpression>? groupBy = null;
        string? groupDescription = null;
        if (Match("GROUP"))
        {
            Expect("BY"); groupBy = [];
            int groupStart = Peek.Position;
            do
            {
                var key = Expression();
                if (key.ContainsAggregate() || SqlNestedQuery.Contains(key)) throw Error("GROUP BY requires scalar expressions over the input row.");
                groupBy.Add(key);
                if (groupBy.Count > 64) throw Error("GROUP BYは64列以下にしてください。");
            } while (Match(","));
            groupDescription = source[groupStart..(End ? source.Length : Peek.Position)].Trim();
        }
        var having = Match("HAVING") ? Expression() : null;
        return new SelectRows(table, projections, where, [], limit, 0, alias, join, groupBy, additionalJoins) { Having = having, Distinct = distinct, DerivedSources = derived, GroupDescription = groupDescription };
    }

    private SelectRows QueryTail(SelectRows query)
    {
        var limit = query.Limit;
        var order = new List<SqlOrder>();
        if (Match("ORDER")) { Expect("BY"); do { order.Add(OrderTerm()); } while (Match(",")); }
        if (Match("LIMIT")) { if (limit is not null) throw Error("TOPとLIMITは併用できません。"); limit = Integer(0, 4096); }
        var offset = 0;
        if (Match("OFFSET"))
        {
            offset = Integer(0, 4096);
            if (!Match("ROW")) Match("ROWS");
        }
        if (Match("FETCH"))
        {
            if (limit.HasValue) throw Error("FETCH cannot be combined with TOP or LIMIT.");
            if (!Match("FIRST")) Expect("NEXT");
            limit = Peek.Kind == "number" ? Integer(0, 4096) : 1;
            if (!Match("ROW")) Expect("ROWS");
            Expect("ONLY");
        }
        return query with { Order = order, Limit = limit, Offset = offset };
    }

    private SqlOrder OrderTerm()
    {
        var field = Field();
        bool descending = Match("DESC");
        if (!descending) Match("ASC");
        bool? nullsFirst = null;
        if (Match("NULLS"))
        {
            nullsFirst = Match("FIRST");
            if (nullsFirst == false) Expect("LAST");
        }
        return new(field.Name, descending, field.Qualifier) { NullsFirst = nullsFirst };
    }

    private string? TableAlias()
    {
        if (inProcedureBody && Is("END")) return null;
        if (Match("AS")) return Name();
        if (Peek.Kind == "identifier") return Name();
        if (Peek.Kind != "word") return null;
        return Peek.Text.ToUpperInvariant() switch
        {
            "INNER" or "JOIN" or "ON" or "WHERE" or "HAVING" or "GROUP" or "ORDER" or "LIMIT" or "OFFSET" or
            "LEFT" or "RIGHT" or "FULL" or "OUTER" or "NATURAL" or "CROSS" or "USING" or "FOR" or
            "WITH" or "UNION" or "ALL" or "INTERSECT" or "EXCEPT" or "FETCH" or "WHEN" or "RETURNING" => null,
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
            if (!(Is("SELECT") || Is("WITH") || Is("INSERT") || Is("MERGE") || Is("UPDATE") || Is("DELETE")))
                throw Error("プロシージャー内にはクエリまたは行の変更を指定してください。");
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

    private SqlValue Literal()
    {
        if (Is("+") || Is("-"))
        {
            var sign = tokens[current++].Text;
            if (Peek.Kind != "number") throw Error("A sign must be followed by a numeric literal.");
            return SqlValue.Number(sign + tokens[current++].Text);
        }
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
            else if (char.IsDigit(c) || c == '.' && i + 1 < sql.Length && char.IsDigit(sql[i + 1])) { i++; while (i < sql.Length && (char.IsDigit(sql[i]) || sql[i] is '.' or 'e' or 'E' || sql[i] is '+' or '-' && sql[i - 1] is 'e' or 'E')) i++; tokens.Add(new(sql[start..i], "number", start)); }
            else if (c == '＋') { i++; tokens.Add(new("+", "symbol", start)); }
            else if ("(),;.*=<>!+-/%".Contains(c)) { i++; if (i < sql.Length && (c is '<' or '>' or '!' && sql[i] == '=' || c == '<' && sql[i] == '>')) i++; tokens.Add(new(sql[start..i], "symbol", start)); }
            else throw new SqlException($"SQL位置 {i + 1}: 未対応の文字 '{c}' です。");
            if (tokens.Count > 50000) throw new SqlException("SQLの要素数が上限を超えています。");
        }
        return tokens;
    }
}
