#nullable enable
using System.Buffers.Binary;
using System.Runtime.InteropServices;
using System.Text;

namespace NewDbms;
public enum NewDbmsConflictKind
{
    Write,
    Read,
    Range
}

public class NewDbmsException : IOException
{
    public int StatusCode { get; }

    internal NewDbmsException(int code, string message) : base(message)
    {
        StatusCode = code;
    }
}

public sealed class NewDbmsConflictException : NewDbmsException
{
    public NewDbmsConflictKind Kind { get; }

    internal NewDbmsConflictException(int code) : base(code, "Transaction conflicted; start a new transaction to retry.")
    {
        Kind = (NewDbmsConflictKind)(code - 10);
    }
}

public sealed class NewDbmsReopenRequiredException : NewDbmsException
{
    public bool OutcomeUncertain => StatusCode is 20 or 25;

    internal NewDbmsReopenRequiredException(int code) : base(code, "Database requires reopen; inspect uncertain outcomes before retrying.")
    {
    }
}

public readonly record struct NewDbmsStats(ulong Version, ulong Records, ulong RetainedVersions, ulong ActiveTransactions, ulong WalBytes);
public readonly record struct NewDbmsCheckpoint(ulong Version, ulong Records, ulong BeforeBytes, ulong AfterBytes);
public readonly record struct NewDbmsRetirementStatus(bool Enabled, ulong MaxBytes, ulong Retained,
    ulong Reclaimed, ulong Fallbacks, ulong LinkNanoseconds, ulong ReclaimNanoseconds, bool HadError);
public readonly record struct NewDbmsExport(ulong Version, ulong Records);
public readonly record struct NewDbmsEntry(byte[] Key, byte[] Value);
public sealed class NewDbmsResourceLimitException : NewDbmsException
{
    internal NewDbmsResourceLimitException() : base(26, "Database resource budget exhausted; release resources or use a larger explicit budget.") { }
}

public sealed record NewDbmsResourceLimits(
    ulong MaxTransactions = 64, ulong MaxStagedBytes = 32 * 1024 * 1024,
    ulong MaxDependencyBytes = 16 * 1024 * 1024, ulong MaxRetainedBytes = 128 * 1024 * 1024,
    ulong MaxBufferBytes = 16 * 1024 * 1024)
{
    internal ulong[] ToArray()
    {
        var values = new[] { MaxTransactions, MaxStagedBytes, MaxDependencyBytes, MaxRetainedBytes, MaxBufferBytes };
        foreach (var value in values)
            if (value == 0 || value > (ulong)nint.MaxValue)
                throw new ArgumentOutOfRangeException(nameof(value));
        return values;
    }
}
public readonly record struct NewDbmsResourceCounters(ulong Transactions, ulong StagedBytes, ulong DependencyBytes, ulong RetainedBytes, ulong BufferBytes);
public readonly record struct NewDbmsResourceUsage(NewDbmsResourceCounters Current, NewDbmsResourceCounters Peak, NewDbmsResourceLimits Limits);
public sealed record NewDbmsScanLimits(int MaxRows = 256, int MaxBytes = 1024 * 1024, int MaxVisits = 4096)
{
    internal void Validate()
    {
        if (MaxRows is < 1 or > 4096 || MaxBytes is < 1 or > 64 * 1024 * 1024 || MaxVisits is < 1 or > 65536)
            throw new ArgumentOutOfRangeException(nameof(NewDbmsScanLimits));
    }
}
public sealed record NewDbmsScanPage(IReadOnlyList<NewDbmsEntry> Entries, byte[]? NextAfter, ulong VisitedEntries, ulong PayloadBytes);
public sealed class NewDbmsDatabase : IDisposable
{
    private readonly DatabaseHandle _handle;
    private readonly object _gate = new();
    public Guid Id { get; }

    private NewDbmsDatabase(ulong id)
    {
        _handle = new DatabaseHandle(id);
        try
        {
            var identity = new byte[16];
            TransactionNative.Check(TransactionNative.ndb_tx_db_info(_handle, identity, new ulong[5]));
            Id = new Guid(identity);
        }
        catch
        {
            _handle.Dispose();
            throw;
        }
    }

    public static NewDbmsDatabase Create(string path, Guid id)
        => Create(path, id, null);

    public static NewDbmsDatabase Create(string path, Guid id, NewDbmsResourceLimits? resources)
    {
        TransactionNative.CheckAbi();
        var bytes = TransactionNative.PathBytes(path);
        ulong handle;
        if (resources == null)
            TransactionNative.Check(TransactionNative.ndb_tx_db_create(bytes, (ulong)bytes.Length, id.ToByteArray(), out handle));
        else
        {
            TransactionNative.CheckResourceApi();
            TransactionNative.Check(TransactionNative.ndb_tx_db_create_limited(bytes, (ulong)bytes.Length, id.ToByteArray(), resources.ToArray(), out handle));
        }
        return new NewDbmsDatabase(handle);
    }

    public static NewDbmsDatabase Open(string path)
        => Open(path, null);

    public static NewDbmsDatabase Open(string path, NewDbmsResourceLimits? resources)
    {
        TransactionNative.CheckAbi();
        var bytes = TransactionNative.PathBytes(path);
        ulong handle;
        if (resources == null)
            TransactionNative.Check(TransactionNative.ndb_tx_db_open(bytes, (ulong)bytes.Length, out handle));
        else
        {
            TransactionNative.CheckResourceApi();
            TransactionNative.Check(TransactionNative.ndb_tx_db_open_limited(bytes, (ulong)bytes.Length, resources.ToArray(), out handle));
        }
        return new NewDbmsDatabase(handle);
    }

    public void SetGroupCommit(bool enabled)
    {
        lock (_gate)
        {
            EnsureOpen();
            try
            {
                TransactionNative.Check(TransactionNative.ndb_tx_set_group_commit(_handle, enabled ? 1u : 0u));
            }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("Native library does not support group commit.", error);
            }
        }
    }

    public void SetCheckpointRetirement(bool enabled, ulong maxBytes = 1024UL * 1024 * 1024)
    {
        if (maxBytes > 1024UL * 1024 * 1024)
            throw new ArgumentOutOfRangeException(nameof(maxBytes));
        lock (_gate)
        {
            EnsureOpen();
            try
            {
                TransactionNative.Check(TransactionNative.ndb_tx_set_checkpoint_retirement(_handle, enabled ? 1u : 0u, maxBytes));
            }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("Native library does not support checkpoint retirement.", error);
            }
        }
    }

    public NewDbmsRetirementStatus GetCheckpointRetirementStatus()
    {
        lock (_gate)
        {
            EnsureOpen();
            try
            {
                var values = new ulong[8];
                TransactionNative.Check(TransactionNative.ndb_tx_checkpoint_retirement_status(_handle, values));
                return new(values[0] != 0, values[1], values[2], values[3], values[4], values[5], values[6], values[7] != 0);
            }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("Native library does not expose checkpoint retirement status.", error);
            }
        }
    }

    public ulong GetCommitSyncAttempts()
    {
        lock (_gate)
        {
            EnsureOpen();
            try
            {
                TransactionNative.Check(TransactionNative.ndb_tx_commit_sync_attempts(_handle, out var attempts));
                return attempts;
            }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("Native library does not expose commit synchronization counts.", error);
            }
        }
    }

    public NewDbmsResourceUsage GetResourceUsage()
    {
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.CheckResourceApi();
            var values = new ulong[15];
            TransactionNative.Check(TransactionNative.ndb_tx_db_resources(_handle, values));
            return new(new(values[0], values[1], values[2], values[3], values[4]),
                new(values[5], values[6], values[7], values[8], values[9]),
                new(values[10], values[11], values[12], values[13], values[14]));
        }
    }

    public NewDbmsTransaction BeginTransaction()
    {
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_begin(_handle, out var handle, out var version));
            return new NewDbmsTransaction(handle, version);
        }
    }

    public NewDbmsStats GetStats()
    {
        lock (_gate)
        {
            EnsureOpen();
            var stats = new ulong[5];
            TransactionNative.Check(TransactionNative.ndb_tx_db_info(_handle, new byte[16], stats));
            return new(stats[0], stats[1], stats[2], stats[3], stats[4]);
        }
    }

    public NewDbmsCheckpoint Checkpoint()
    {
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_checkpoint(_handle, out var version, out var records, out var before, out var after));
            return new(version, records, before, after);
        }
    }

    public NewDbmsExport Backup(string destination)
    {
        var path = TransactionNative.PathBytes(destination);
        lock (_gate)
        {
            EnsureOpen();
            try
            {
                TransactionNative.Check(TransactionNative.ndb_tx_backup(_handle, path, (ulong)path.Length, out var version, out var count));
                return new(version, count);
            }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("The native catdb library does not support online backup.", error);
            }
        }
    }

    public static NewDbmsExport Restore(string source, string destination)
    {
        TransactionNative.CheckAbi();
        var backup = TransactionNative.PathBytes(source);
        var path = TransactionNative.PathBytes(destination);
        try
        {
            TransactionNative.Check(TransactionNative.ndb_tx_restore(backup, (ulong)backup.Length, path, (ulong)path.Length, out var version, out var count));
            return new(version, count);
        }
        catch (EntryPointNotFoundException error)
        {
            throw new NotSupportedException("The native catdb library does not support backup restore.", error);
        }
    }

    public NewDbmsCheckpoint CheckpointDeferredReclaim()
    {
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_checkpoint_deferred(_handle, out var version, out var records, out var before, out var after));
            return new(version, records, before, after);
        }
    }

    public void ReclaimCheckpoint()
    {
        bool held = false;
        ulong id;
        lock (_gate)
        {
            EnsureOpen();
            _handle.DangerousAddRef(ref held);
            id = unchecked((ulong)_handle.DangerousGetHandle().ToInt64());
        }
        try
        {
            TransactionNative.Check(TransactionNative.ndb_tx_reclaim_checkpoint(id));
        }
        finally
        {
            if (held) _handle.DangerousRelease();
        }
    }

    public NewDbmsExport ExportValues(string path, Guid generation, byte[] start, byte[]? end = null)
    {
        var bytes = TransactionNative.PathBytes(path);
        TransactionNative.Key(start);
        if (end != null)
            TransactionNative.Key(end);
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_export(_handle, bytes, (ulong)bytes.Length, generation.ToByteArray(), start, (ulong)start.Length, end ?? Array.Empty<byte>(), (ulong)(end?.Length ?? 0), end == null ? 0u : 1u, out var version, out var count));
            return new(version, count);
        }
    }

    private void EnsureOpen() => ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
    public void Dispose()
    {
        lock (_gate)
            _handle.Dispose();
    }
}

public sealed class NewDbmsTransaction : IDisposable
{
    private readonly TransactionHandle _handle;
    private readonly object _gate = new();
    public ulong SnapshotVersion { get; }

    internal NewDbmsTransaction(ulong id, ulong version)
    {
        _handle = new TransactionHandle(id);
        SnapshotVersion = version;
    }

    public byte[]? Get(byte[] key)
    {
        TransactionNative.Key(key);
        lock (_gate)
        {
            EnsureOpen();
            var buffer = new byte[4096];
            var code = TransactionNative.ndb_tx_get(_handle, key, (ulong)key.Length, buffer, (ulong)buffer.Length, out var bytes, out var found);
            if (code == 3)
            {
                buffer = TransactionNative.Buffer(bytes, TransactionNative.MaxValueBytes);
                code = TransactionNative.ndb_tx_get(_handle, key, (ulong)key.Length, buffer, (ulong)buffer.Length, out bytes, out found);
            }

            TransactionNative.Check(code);
            if (found > 1 || bytes > (ulong)buffer.Length || (found == 0 && bytes != 0))
                throw new InvalidDataException("Invalid native value response.");
            return found == 0 ? null : buffer.AsSpan(0, checked((int)bytes)).ToArray();
        }
    }

    public void Put(byte[] key, byte[] value)
    {
        TransactionNative.Key(key);
        ArgumentNullException.ThrowIfNull(value);
        if (value.Length > TransactionNative.MaxValueBytes)
            throw new ArgumentOutOfRangeException(nameof(value));
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_put(_handle, key, (ulong)key.Length, value, (ulong)value.Length));
        }
    }

    public void Delete(byte[] key)
    {
        TransactionNative.Key(key);
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_delete(_handle, key, (ulong)key.Length));
        }
    }

    public void CreateSavepoint(string name) => ChangeSavepoint(name, 0);
    public void LockKeys(IReadOnlyList<byte[]> keys)
    {
        ArgumentNullException.ThrowIfNull(keys);
        if (keys.Count > 4096) throw new ArgumentOutOfRangeException(nameof(keys));
        using var stream = new MemoryStream();
        Span<byte> length = stackalloc byte[2];
        foreach (var key in keys)
        {
            TransactionNative.Key(key);
            BinaryPrimitives.WriteUInt16LittleEndian(length, checked((ushort)key.Length));
            stream.Write(length);
            stream.Write(key);
        }
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.Check(TransactionNative.ndb_tx_lock_keys(_handle, stream.ToArray(), (ulong)stream.Length));
        }
    }
    public void RollbackToSavepoint(string name) => ChangeSavepoint(name, 1);
    public void ReleaseSavepoint(string name) => ChangeSavepoint(name, 2);

    private void ChangeSavepoint(string name, uint operation)
    {
        var bytes = TransactionNative.SavepointName(name);
        lock (_gate)
        {
            EnsureOpen();
            try { TransactionNative.Check(TransactionNative.ndb_tx_savepoint(_handle, operation, bytes, (ulong)bytes.Length)); }
            catch (EntryPointNotFoundException error)
            {
                throw new NotSupportedException("The loaded catdb native library does not support savepoints.", error);
            }
        }
    }

    public IReadOnlyList<NewDbmsEntry> Scan(byte[] start, byte[]? end = null)
    {
        TransactionNative.Key(start);
        if (end != null)
            TransactionNative.Key(end);
        lock (_gate)
        {
            EnsureOpen();
            var buffer = new byte[1024 * 1024];
            int Read(out ulong bytes, out ulong count) => TransactionNative.ndb_tx_scan(_handle, start, (ulong)start.Length, end ?? Array.Empty<byte>(), (ulong)(end?.Length ?? 0), end == null ? 0u : 1u, buffer, (ulong)buffer.Length, out bytes, out count);
            var code = Read(out var length, out var count);
            if (code == 3)
            {
                buffer = TransactionNative.Buffer(length, 64 * 1024 * 1024);
                code = Read(out length, out count);
            }

            TransactionNative.Check(code);
            if (length > (ulong)buffer.Length || count > 1_000_000)
                throw new InvalidDataException("Oversized native scan response.");
            var result = new List<NewDbmsEntry>(checked((int)count));
            var position = 0;
            for (ulong i = 0; i < count; i++)
            {
                if ((ulong)position + 8 > length)
                    throw new InvalidDataException("Truncated native scan frame.");
                var k = BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(position, 4));
                var v = BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(position + 4, 4));
                position += 8;
                if (k > 1024 || v > TransactionNative.MaxValueBytes || (ulong)position + k + v > length)
                    throw new InvalidDataException("Invalid native scan frame.");
                var key = buffer.AsSpan(position, (int)k).ToArray();
                position += (int)k;
                var value = buffer.AsSpan(position, (int)v).ToArray();
                position += (int)v;
                result.Add(new(key, value));
            }

            if ((ulong)position != length)
                throw new InvalidDataException("Trailing native scan bytes.");
            return result;
        }
    }

    public NewDbmsScanPage ScanPage(byte[] start, byte[]? end = null, byte[]? after = null, NewDbmsScanLimits? limits = null)
    {
        TransactionNative.Key(start);
        if (end != null) TransactionNative.Key(end);
        if (after != null) TransactionNative.Key(after);
        limits ??= new();
        limits.Validate();
        lock (_gate)
        {
            EnsureOpen();
            TransactionNative.CheckResourceApi();
            var maximum = checked(limits.MaxBytes + limits.MaxRows * 8 + 1064);
            var buffer = new byte[Math.Min(maximum, 64 * 1024)];
            int Read(out ulong required) => TransactionNative.ndb_tx_scan_page(_handle,
                start, (ulong)start.Length, end ?? Array.Empty<byte>(), (ulong)(end?.Length ?? 0), end == null ? 0u : 1u,
                after ?? Array.Empty<byte>(), (ulong)(after?.Length ?? 0), after == null ? 0u : 1u,
                (ulong)limits.MaxRows, (ulong)limits.MaxBytes, (ulong)limits.MaxVisits,
                buffer, (ulong)buffer.Length, out required);
            var code = Read(out var length);
            if (code == 3)
            {
                buffer = TransactionNative.Buffer(length, maximum);
                code = Read(out length);
            }
            TransactionNative.Check(code);
            if (length < 40 || length > (ulong)buffer.Length || length > (ulong)maximum)
                throw new InvalidDataException("Invalid native page length.");
            ulong Header(int index) => BinaryPrimitives.ReadUInt64LittleEndian(buffer.AsSpan(index * 8, 8));
            var count = Header(0); var visited = Header(1); var payload = Header(2);
            var hasNext = Header(3); var cursorLength = Header(4);
            if (count > (ulong)limits.MaxRows || visited > (ulong)limits.MaxVisits || payload > (ulong)limits.MaxBytes
                || hasNext > 1 || cursorLength > 1024 || (hasNext == 0 && cursorLength != 0) || 40 + cursorLength > length)
                throw new InvalidDataException("Invalid native page header.");
            var position = checked(40 + (int)cursorLength);
            byte[]? next = hasNext == 0 ? null : buffer.AsSpan(40, (int)cursorLength).ToArray();
            var entries = new List<NewDbmsEntry>((int)count);
            ulong actualPayload = 0;
            for (ulong index = 0; index < count; index++)
            {
                if ((ulong)position + 8 > length) throw new InvalidDataException("Truncated native page.");
                var k = BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(position, 4));
                var v = BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(position + 4, 4));
                position += 8;
                if (k > 1024 || v > TransactionNative.MaxValueBytes || (ulong)position + k + v > length)
                    throw new InvalidDataException("Invalid native page entry.");
                var key = buffer.AsSpan(position, (int)k).ToArray(); position += (int)k;
                var value = buffer.AsSpan(position, (int)v).ToArray(); position += (int)v;
                entries.Add(new(key, value)); actualPayload += k + v;
            }
            if ((ulong)position != length || actualPayload != payload)
                throw new InvalidDataException("Invalid native page payload.");
            return new(entries, next, visited, payload);
        }
    }

    public ulong Commit()
    {
        lock (_gate)
        {
            EnsureOpen();
            var code = TransactionNative.ndb_tx_commit(_handle, out var version);
            _handle.SetHandleAsInvalid();
            TransactionNative.Check(code);
            return version;
        }
    }

    public void Rollback() => Dispose();
    private void EnsureOpen() => ObjectDisposedException.ThrowIf(_handle.IsClosed, this);
    public void Dispose()
    {
        lock (_gate)
            _handle.Dispose();
    }
}

internal sealed class DatabaseHandle : SafeHandle
{
    internal DatabaseHandle(ulong id) : base(IntPtr.Zero, true)
    {
        SetHandle(unchecked((IntPtr)(long)id));
    }

    public override bool IsInvalid => handle == IntPtr.Zero;

    protected override bool ReleaseHandle() => TransactionNative.ndb_tx_db_close(unchecked((ulong)handle.ToInt64())) == 0;
}

internal sealed class TransactionHandle : SafeHandle
{
    internal TransactionHandle(ulong id) : base(IntPtr.Zero, true)
    {
        SetHandle(unchecked((IntPtr)(long)id));
    }

    public override bool IsInvalid => handle == IntPtr.Zero;

    protected override bool ReleaseHandle() => TransactionNative.ndb_tx_rollback(unchecked((ulong)handle.ToInt64())) == 0;
}

internal static class TransactionNative
{
    internal const int MaxValueBytes = 16 * 1024 * 1024 - 1028;
    private static readonly Encoding Utf8 = new UTF8Encoding(false, true);
    internal static void CheckResourceApi()
    {
        try
        {
            if (ndb_tx_resource_api_version() != 1) throw new NotSupportedException("Unsupported resource/page API.");
        }
        catch (EntryPointNotFoundException error)
        {
            throw new NotSupportedException("Native library does not support resource limits or paged scans.", error);
        }
    }
    internal static void CheckAbi()
    {
        if (IntPtr.Size != 8)
            throw new PlatformNotSupportedException("Transaction ABI requires a 64-bit process.");
        if (ndb_tx_abi_version() != 1)
            throw new NotSupportedException("Unsupported NewDBMS transaction ABI.");
    }

    internal static byte[] PathBytes(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        return Utf8.GetBytes(Path.GetFullPath(path));
    }

    internal static byte[] SavepointName(string name)
    {
        ArgumentException.ThrowIfNullOrEmpty(name);
        if (Utf8.GetByteCount(name) > 512) throw new ArgumentOutOfRangeException(nameof(name));
        return Utf8.GetBytes(name);
    }

    internal static void Key(byte[] key)
    {
        ArgumentNullException.ThrowIfNull(key);
        if (key.Length > 1024)
            throw new ArgumentOutOfRangeException(nameof(key));
    }

    internal static byte[] Buffer(ulong bytes, int maximum)
    {
        if (bytes > (ulong)maximum)
            throw new InvalidDataException("Oversized native response.");
        return new byte[checked((int)bytes)];
    }

    internal static void Check(int code)
    {
        if (code == 0)
            return;
        if (code is >= 10 and <= 12)
            throw new NewDbmsConflictException(code);
        if (code is 20 or 21 or 25)
            throw new NewDbmsReopenRequiredException(code);
        if (code == 26) throw new NewDbmsResourceLimitException();
        throw new NewDbmsException(code, $"NewDBMS transaction operation failed (status {code}).");
    }

    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint ndb_tx_abi_version();
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern uint ndb_tx_resource_api_version();
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_set_group_commit(DatabaseHandle handle, uint enabled);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_set_checkpoint_retirement(DatabaseHandle handle, uint enabled, ulong maxBytes);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_checkpoint_retirement_status(DatabaseHandle handle, [Out] ulong[] values);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_commit_sync_attempts(DatabaseHandle handle, out ulong attempts);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_create_limited(byte[] path, ulong length, byte[] identity, ulong[] limits, out ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_open_limited(byte[] path, ulong length, ulong[] limits, out ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_resources(DatabaseHandle handle, [Out] ulong[] values);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_scan_page(TransactionHandle handle, byte[] start, ulong startLength,
        byte[] end, ulong endLength, uint hasEnd, byte[] after, ulong afterLength, uint hasAfter,
        ulong maxRows, ulong maxBytes, ulong maxVisits, [Out] byte[] buffer, ulong capacity, out ulong required);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_create(byte[] path, ulong length, byte[] identity, out ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_open(byte[] path, ulong length, out ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_close(ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_db_info(DatabaseHandle handle, [Out] byte[] identity, [Out] ulong[] stats);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_begin(DatabaseHandle handle, out ulong transaction, out ulong version);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_get(TransactionHandle handle, byte[] key, ulong keyLength, [Out] byte[] buffer, ulong capacity, out ulong required, out ulong found);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_put(TransactionHandle handle, byte[] key, ulong keyLength, byte[] value, ulong valueLength);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_delete(TransactionHandle handle, byte[] key, ulong keyLength);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_lock_keys(TransactionHandle handle, byte[] frame, ulong length);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_savepoint(TransactionHandle handle, uint operation, byte[] name, ulong nameLength);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_scan(TransactionHandle handle, byte[] start, ulong startLength, byte[] end, ulong endLength, uint hasEnd, [Out] byte[] buffer, ulong capacity, out ulong required, out ulong count);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_commit(TransactionHandle handle, out ulong version);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_rollback(ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_checkpoint(DatabaseHandle handle, out ulong version, out ulong records, out ulong before, out ulong after);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_backup(DatabaseHandle handle, byte[] destination, ulong destinationLength, out ulong version, out ulong records);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_restore(byte[] source, ulong sourceLength, byte[] destination, ulong destinationLength, out ulong version, out ulong records);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_checkpoint_deferred(DatabaseHandle handle, out ulong version, out ulong records, out ulong before, out ulong after);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_reclaim_checkpoint(ulong handle);
    [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
    internal static extern int ndb_tx_export(DatabaseHandle handle, byte[] path, ulong length, byte[] generation, byte[] start, ulong startLength, byte[] end, ulong endLength, uint hasEnd, out ulong version, out ulong count);
}
