using System.Text.Json.Nodes;
using Catdb.Sql;

namespace Catdb.Owner.Services;

public sealed class MemorySqlStore
{
    private SortedDictionary<string, string> data = new(StringComparer.Ordinal);
    private SortedDictionary<string, string>? transaction;
    public bool InTransaction => transaction is not null;
    public Task<JsonObject> RequestAsync(JsonObject request)
    {
        var result = new JsonObject { ["ok"] = true };
        var op = request["op"]!.ToString();
        if (op == "begin")
        {
            if (transaction is not null) throw new CatdbRequestException("トランザクションは既に開始されています。");
            transaction = new(data, StringComparer.Ordinal); return Task.FromResult(result);
        }
        if (transaction is null) throw new CatdbRequestException("トランザクションがありません。");
        switch (op)
        {
            case "rollback": transaction = null; break;
            case "commit": data = transaction; transaction = null; break;
            case "get": result["value"] = transaction.GetValueOrDefault(request["key"]!.ToString().ToUpperInvariant()); break;
            case "put": transaction[request["key"]!.ToString().ToUpperInvariant()] = request["value"]!.ToString().ToUpperInvariant(); break;
            case "delete": transaction.Remove(request["key"]!.ToString().ToUpperInvariant()); break;
            case "scan":
                var start = request["start"]!.ToString().ToUpperInvariant(); var end = request["end"]?.ToString().ToUpperInvariant(); var after = request["after"]?.ToString().ToUpperInvariant();
                var candidates = transaction.Where(x => string.CompareOrdinal(x.Key, start) >= 0 && (end is null || string.CompareOrdinal(x.Key, end) < 0) && (after is null || string.CompareOrdinal(x.Key, after) > 0)).ToList();
                var page = new JsonArray(); var bytes = 0; var limit = request["rows"]?.GetValue<int>() ?? 100; var byteLimit = request["bytes"]?.GetValue<int>() ?? 65536;
                foreach (var item in candidates)
                {
                    var size = (item.Key.Length + item.Value.Length) / 2;
                    if (page.Count >= limit || bytes + size > byteLimit) break;
                    page.Add(new JsonArray(item.Key, item.Value)); bytes += size;
                }
                if (page.Count == 0 && candidates.Count > 0) throw new CatdbRequestException("limit");
                result["next_after"] = page.Count < candidates.Count ? candidates[page.Count - 1].Key : null;
                result["rows"] = page; break;
            default: throw new CatdbRequestException("未対応の操作です。");
        }
        return Task.FromResult(result);
    }
}
