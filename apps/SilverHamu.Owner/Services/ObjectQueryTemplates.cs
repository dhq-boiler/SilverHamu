using SilverHamu.Sql;

namespace SilverHamu.Owner.Services;

internal sealed record ObjectQueryTemplate(string Operation, string Caption, string Sql, bool EditResults = false);

internal static class ObjectQueryTemplates
{
    public static IReadOnlyList<ObjectQueryTemplate> Create(SqlTable table)
    {
        string name = SqlIdentifiers.Format(table.Name);
        var select = new ObjectQueryTemplate("Select", L.Get("SelectFirstThousandRows"), $"SELECT *\nFROM {name}\nFETCH FIRST 1000 ROWS ONLY;");
        var selectAll = new ObjectQueryTemplate("SelectAll", L.Get("SelectAllRows"), $"SELECT *\nFROM {name};");
        if (table.ViewQuery is not null) return [select, selectAll];

        string hint = L.Get("TemplateInstructions");
        string columns = table.Columns.Count == 0 ? L.Get("EnterColumn") :
            string.Join(",\n    ", table.Columns.Select(c => SqlIdentifiers.Format(c.Name)));
        string values = table.Columns.Count == 0 ? L.Get("EnterValue") :
            string.Join(",\n    ", table.Columns.Select((_, i) => L.Get("TemplateValue", i + 1)));
        var keys = table.CompositePrimaryKey ?? table.Columns.Where(c => c.PrimaryKey).Select(c => c.Name).ToList();
        string predicate = keys.Count == 0 ? L.Get("EnterWHERECondition") :
            string.Join("\n    AND ", keys.Select((key, i) => L.Get("TemplateKeyPredicate", SqlIdentifiers.Format(key), i + 1)));
        var writable = table.Columns.Where(c => !keys.Contains(c.Name, StringComparer.OrdinalIgnoreCase)).ToArray();
        if (writable.Length == 0) writable = table.Columns.ToArray();
        string assignments = writable.Length == 0 ? L.Get("EnterAssignment") :
            string.Join(",\n    ", writable.Select((c, i) => L.Get("TemplateAssignment", SqlIdentifiers.Format(c.Name), i + 1)));
        return
        [
            select,
            selectAll,
            new("SelectEdit", L.Get("SelectThousandAndEdit"), select.Sql, EditResults: true),
            new("SelectAllEdit", L.Get("SelectAllAndEdit"), selectAll.Sql, EditResults: true),
            new("Insert", "INSERT", hint + $"INSERT INTO {name} (\n    {columns}\n)\nVALUES (\n    {values}\n);"),
            new("Update", "UPDATE", hint + $"UPDATE {name}\nSET\n    {assignments}\nWHERE\n    {predicate};"),
            new("Delete", "DELETE", hint + $"DELETE FROM {name}\nWHERE\n    {predicate};")
        ];
    }
}
