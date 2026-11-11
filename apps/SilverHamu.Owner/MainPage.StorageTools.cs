using SilverHamu.Owner.Services;
using System.Text.Json.Nodes;
using SilverHamu.Sql;

namespace SilverHamu.Owner;

public sealed partial class MainPage
{
    private readonly Button rawKvButton;
    private readonly Button snapshotButton;
    private bool CanUseRawStore => !IsReadRole && (embedded is not null || client is not null);

    private async Task<string?> AskHexAsync(string title, string label, int maxBytes)
    {
        var text = await DisplayPromptAsync(title, label, L.Get("OK"), L.Get("Cancel"), maxLength: maxBytes * 2);
        if (text is null) return null;
        try { return Convert.ToHexString(Convert.FromHexString(text.Trim())); }
        catch (FormatException) { throw new InvalidOperationException(L.Get("EnterHexadecimalDigitsInPairs")); }
    }

    private async Task<(string Start, string? End)?> AskRangeAsync(string title)
    {
        var start = await AskHexAsync(title, L.Get("StartKeyHexInclusiveLeaveBlankToStart"), 1024);
        if (start is null) return null;
        var end = await AskHexAsync(title, L.Get("EndKeyHexExclusiveLeaveBlankToInclude"), 1024);
        if (end is null) return null;
        if (end.Length > 0 && string.CompareOrdinal(start, end) > 0)
            throw new InvalidOperationException(L.Get("TheEndKeyMustBeAtOrAfter"));
        return (start, end.Length == 0 ? null : end);
    }

    private async Task RawKvAsync()
    {
        if (!CanUseRawStore) throw new InvalidOperationException(L.Get("AnAdministratorDBConnectionIsRequired"));
        var op = await DisplayActionSheetAsync(L.Get("KeyValueTools144"), L.Get("Cancel"), null, "GET", "SCAN", "PUT", "DELETE");
        if (op is not ("GET" or "SCAN" or "PUT" or "DELETE")) return;
        var request = new JsonObject { ["op"] = op.ToLowerInvariant() };
        var write = op is "PUT" or "DELETE";
        if (op == "SCAN")
        {
            var range = await AskRangeAsync(L.Get("SCANUpToRecordsKiB"));
            if (range is null) return;
            request["start"] = range.Value.Start;
            request["end"] = range.Value.End;
            request["rows"] = 100;
            request["bytes"] = 65536;
            request["visits"] = 65536;
        }
        else
        {
            var key = await AskHexAsync(op, L.Get("KeyHex"), 1024);
            if (key is null) return;
            if (write && Convert.FromHexString(key).AsSpan().StartsWith("_catdb_owner/"u8))
                throw new InvalidOperationException(L.Get("UseSQLToModifyKeysInTheSQL"));
            request["key"] = key;
            if (op == "PUT")
            {
                var value = await AskHexAsync(L.Get("PUTInsertUpdate"), L.Get("ValueHexBlankMeansAZeroLengthValue"), 256 * 1024);
                if (value is null) return;
                request["value"] = value;
            }
        }

        var ownTransaction = !inTransaction;
        if (ownTransaction) await SendAsync(new JsonObject { ["op"] = "begin" });
        JsonObject response;
        try
        {
            response = await SendAsync(request);
            if (ownTransaction) await SendAsync(new JsonObject { ["op"] = write ? "commit" : "rollback" });
        }
        catch
        {
            if (ownTransaction && inTransaction) await SendAsync(new JsonObject { ["op"] = "rollback" });
            throw;
        }
        if (op == "SCAN")
        {
            var rows = response["rows"]!.AsArray().Select(pair => new[] {
                new SqlValue("BLOB", pair![0]!.ToString()), new SqlValue("BLOB", pair[1]!.ToString())
            }).ToArray();
            result = new(["key", "value"], rows, response["next_after"] is null ? L.Get("Records", rows.Length) : L.Get("RecordsMoreAvailable", rows.Length));
        }
        else if (op == "GET")
        {
            var value = response["value"];
            result = new(["value"], value is null ? [] : [new[] { new SqlValue("BLOB", value.ToString()) }], value is null ? L.Get("KeyNotFound") : L.Get("Record"));
        }
        else result = new([], [], ownTransaction ? L.Get("Committed", op) : L.Get("ChangesFromAreUncommitted", op));
        filter.Text = "";
        RenderRows();
        Log(result.Message);
    }

    private async Task ExportSnapshotAsync()
    {
        var store = embedded;
        if (store is null || IsReadRole) throw new InvalidOperationException(L.Get("OpenALocalEmbeddedDB"));
        var range = await AskRangeAsync(L.Get("ExportCommittedValuesInRangeToASnapshot"));
        if (range is null) return;
        var path = await PickPathAsync(L.Get("SaveASnapshotOfValuesWithoutKeys"), Path.ChangeExtension(store.DatabasePath, ".snapshot"), true);
        if (path is null) return;
        var generation = Guid.NewGuid();
        var info = await store.ExportSnapshotAsync(path, generation, Convert.FromHexString(range.Value.Start),
            range.Value.End is null ? null : Convert.FromHexString(range.Value.End));
        Log(L.Get("SavedValuesSnapshotRecordsDBVersionGenerationID", info.Records, info.Version, generation));
    }
}
