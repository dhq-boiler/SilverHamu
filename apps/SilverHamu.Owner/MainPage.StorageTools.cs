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
        var text = await DisplayPromptAsync(title, label, "決定", "キャンセル", maxLength: maxBytes * 2);
        if (text is null) return null;
        try { return Convert.ToHexString(Convert.FromHexString(text.Trim())); }
        catch (FormatException) { throw new InvalidOperationException("16進数を2桁ずつ入力してください。"); }
    }

    private async Task<(string Start, string? End)?> AskRangeAsync(string title)
    {
        var start = await AskHexAsync(title, "開始キー（16進・この値を含む）。空欄で先頭から。", 1024);
        if (start is null) return null;
        var end = await AskHexAsync(title, "終了キー（16進・この値を含まない）。空欄で末尾まで。", 1024);
        if (end is null) return null;
        if (end.Length > 0 && string.CompareOrdinal(start, end) > 0)
            throw new InvalidOperationException("終了キーは開始キー以降を指定してください。");
        return (start, end.Length == 0 ? null : end);
    }

    private async Task RawKvAsync()
    {
        if (!CanUseRawStore) throw new InvalidOperationException("管理者のDB接続が必要です。");
        var op = await DisplayActionSheetAsync("キー・値の操作", "キャンセル", null, "GET", "SCAN", "PUT", "DELETE");
        if (op is not ("GET" or "SCAN" or "PUT" or "DELETE")) return;
        var request = new JsonObject { ["op"] = op.ToLowerInvariant() };
        var write = op is "PUT" or "DELETE";
        if (op == "SCAN")
        {
            var range = await AskRangeAsync("SCAN（最大100件・64 KiB）");
            if (range is null) return;
            request["start"] = range.Value.Start;
            request["end"] = range.Value.End;
            request["rows"] = 100;
            request["bytes"] = 65536;
            request["visits"] = 65536;
        }
        else
        {
            var key = await AskHexAsync(op, "キー（16進）", 1024);
            if (key is null) return;
            if (write && Convert.FromHexString(key).AsSpan().StartsWith("_catdb_owner/"u8))
                throw new InvalidOperationException("SQL管理領域のキーはSQLから変更してください。");
            request["key"] = key;
            if (op == "PUT")
            {
                var value = await AskHexAsync("PUT（登録・更新）", "値（16進）。空欄は長さ0の値です。", 256 * 1024);
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
            result = new(["key", "value"], rows, response["next_after"] is null ? $"{rows.Length} 件" : $"{rows.Length} 件（続きあり）");
        }
        else if (op == "GET")
        {
            var value = response["value"];
            result = new(["value"], value is null ? [] : [new[] { new SqlValue("BLOB", value.ToString()) }], value is null ? "キーが見つかりません。" : "1 件");
        }
        else result = new([], [], ownTransaction ? $"{op} を確定しました。" : $"{op} の変更は未確定です。");
        filter.Text = "";
        RenderRows();
        Log(result.Message);
    }

    private async Task ExportSnapshotAsync()
    {
        var store = embedded;
        if (store is null || IsReadRole) throw new InvalidOperationException("ローカルの組み込みDBを開いてください。");
        var range = await AskRangeAsync("確定済み範囲の値をスナップショットへ出力");
        if (range is null) return;
        var path = await PickPathAsync("キーを除く値のスナップショットを保存", Path.ChangeExtension(store.DatabasePath, ".snapshot"), true);
        if (path is null) return;
        var generation = Guid.NewGuid();
        var info = await store.ExportSnapshotAsync(path, generation, Convert.FromHexString(range.Value.Start),
            range.Value.End is null ? null : Convert.FromHexString(range.Value.End));
        Log($"値のスナップショットを保存しました（{info.Records} 件、DBバージョン {info.Version}、世代ID {generation}）。");
    }
}
