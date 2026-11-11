namespace SilverHamu.Owner.Services;

internal static class SqlIdentifiers
{
    public static string Format(string name) => name.Length > 0 &&
        (char.IsLetter(name[0]) || name[0] == '_') && name.All(c => char.IsLetterOrDigit(c) || c == '_') &&
        !SqlReservedWords.Contains(name) ? name : Quote(name);

    public static string Quote(string name) => "\"" + name.Replace("\"", "\"\"") + "\"";
}
