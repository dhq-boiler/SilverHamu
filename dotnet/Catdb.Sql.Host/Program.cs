using System.Text;
using System.Text.Json;
using Catdb.Sql;
using Catdb.Sql.Host;

Console.OutputEncoding = new UTF8Encoding(false);
try
{
    if (args is ["bridge"])
    {
        using var input = new BufferedStream(Console.OpenStandardInput());
        using var output = Console.OpenStandardOutput();
        await new SqlBridge(input, output).RunAsync();
        return 0;
    }
    if (args is ["--help"] or ["-h"])
    {
        Console.WriteLine("Catdb.Sql.Host [--create] [--json] DB [SQL]\nIf SQL is omitted, reads a UTF-8 SQL script from stdin.");
        return 0;
    }

    int pos = 0;
    bool create = false, json = false;
    while (pos < args.Length && args[pos].StartsWith("--", StringComparison.Ordinal))
    {
        var option = args[pos++];
        if (option == "--") break;
        if (option == "--create") create = true;
        else if (option == "--json") json = true;
        else throw new ArgumentException($"Unknown option {option}");
    }
    if (args.Length - pos is < 1 or > 2)
        throw new ArgumentException("Usage: Catdb.Sql.Host [--create] [--json] DB [SQL]");
    var path = Path.GetFullPath(args[pos++]);
    var sql = pos < args.Length ? args[pos] : ReadScript();
    using var connection = await CatdbSqlConnection.OpenAsync(path, create);
    var results = await connection.ExecuteAsync(sql);
    if (json)
    {
        JsonSerializer.Serialize(Console.OpenStandardOutput(), results);
        Console.WriteLine();
    }
    else
    {
        foreach (var result in results)
        {
            if (result.Columns.Count > 0) Console.WriteLine(string.Join('\t', result.Columns));
            foreach (var row in result.Rows)
                Console.WriteLine(string.Join('\t', row.Select(v => v.Display)));
            Console.WriteLine(result.Message);
        }
    }
    if (connection.InTransaction)
        Console.Error.WriteLine("Uncommitted transaction rolled back on close.");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(error.Message);
    return 1;
}

static string ReadScript()
{
    using var reader = new StreamReader(Console.OpenStandardInput(), new UTF8Encoding(false, true), detectEncodingFromByteOrderMarks: true);
    const int limit = 1024 * 1024;
    var text = new char[limit + 1];
    int count = reader.ReadBlock(text, 0, text.Length);
    if (count > limit) throw new InvalidDataException("SQL script exceeds 1 Mi characters.");
    return new string(text, 0, count);
}
