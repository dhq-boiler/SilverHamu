using System.Text;

namespace SilverHamu.Owner.Services;

internal static class Catlink
{
    public static string Resolve(string path)
    {
        var root = Path.GetDirectoryName(Path.GetFullPath(path))!;
        RejectReparsePoints(root, path);
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length > 4096) throw new InvalidDataException(L.Get("TheReferenceFileMustBeKiBOrSmaller"));
        using var reader = new StreamReader(stream, new UTF8Encoding(false, true), detectEncodingFromByteOrderMarks: true);
        var relative = reader.ReadToEnd().Trim();
        if (relative.StartsWith(@"\\") || relative.StartsWith("//") || relative.Contains(':') || relative.Any(char.IsControl))
            throw new InvalidDataException(L.Get("EnterARelativeTargetPathOnASingle"));
        // Older directory pointers may start with a single separator.
        relative = relative.TrimStart('\\', '/');
        var target = Path.GetFullPath(Path.Combine(root, relative));
        var withinRoot = Path.GetRelativePath(root, target);
        if (withinRoot is "." or ".." || withinRoot.StartsWith(".." + Path.DirectorySeparatorChar) || Path.IsPathRooted(withinRoot))
            throw new InvalidDataException(L.Get("TheTargetIsOutsideTheReferenceFileS"));
        RejectReparsePoints(root, target);
        if (!Directory.Exists(target) && !Path.GetExtension(target).Equals(".ndb", StringComparison.OrdinalIgnoreCase))
            throw new InvalidDataException(L.Get("TheTargetMustBeASilverhamuDatabaseOr"));
        return target;
    }

    private static void RejectReparsePoints(string root, string path)
    {
        for (string? current = Path.GetFullPath(path); current is not null; current = Path.GetDirectoryName(current))
        {
            if ((File.GetAttributes(current) & FileAttributes.ReparsePoint) != 0)
                throw new InvalidDataException(L.Get("LinksAndReparsePointsCannotBeUsedIn"));
            if (current.Equals(root, StringComparison.OrdinalIgnoreCase)) break;
        }
    }
}
