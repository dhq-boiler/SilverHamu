using System.Runtime.InteropServices;
using System.Text;

namespace NewDbms;

public sealed class NewDbmsSnapshot : IDisposable
{
    private ulong _handle;
    public ulong Count { get; }

    public NewDbmsSnapshot(string path, byte[] generation)
    {
        if (generation.Length != 16) throw new ArgumentException("Generation must be 16 bytes.");
        if (Native.ndb_abi_version() != 1) throw new NotSupportedException("Unsupported NewDBMS ABI.");
        var encoded = Encoding.UTF8.GetBytes(path);
        Check(Native.ndb_open(encoded, (ulong)encoded.Length, generation, out _handle, out var count));
        Count = count;
    }

    public static void Create(string path, byte[] generation, byte[] framed, ulong count)
    {
        if (generation.Length != 16) throw new ArgumentException("Generation must be 16 bytes.");
        if (Native.ndb_abi_version() != 1) throw new NotSupportedException("Unsupported NewDBMS ABI.");
        var encoded = Encoding.UTF8.GetBytes(path);
        Check(Native.ndb_create(encoded, (ulong)encoded.Length, generation, framed, (ulong)framed.Length, count));
    }

    public byte[][] ReadPage(ulong start, uint limit)
    {
        ObjectDisposedException.ThrowIf(_handle == 0, this);
        var buffer = new byte[1024 * 1024];
        var code = Native.ndb_read_page(_handle, start, limit, buffer, (ulong)buffer.Length, out var bytes, out var count);
        if (code == 3)
        {
            if (bytes > 64 * 1024 * 1024 + 4096 * 4) throw new InvalidDataException("Oversized native page.");
            buffer = new byte[checked((int)bytes)];
            code = Native.ndb_read_page(_handle, start, limit, buffer, (ulong)buffer.Length, out bytes, out count);
        }
        Check(code);
        if (bytes > (ulong)buffer.Length || count > 4096) throw new InvalidDataException("Invalid native page.");
        var records = new byte[checked((int)count)][];
        var position = 0;
        for (var i = 0; i < records.Length; i++)
        {
            if (position + 4 > (int)bytes) throw new InvalidDataException("Truncated frame.");
            var length = System.Buffers.Binary.BinaryPrimitives.ReadUInt32LittleEndian(buffer.AsSpan(position, 4));
            position += 4;
            if (length > bytes - (ulong)position) throw new InvalidDataException("Invalid frame length.");
            records[i] = buffer.AsSpan(position, checked((int)length)).ToArray();
            position += (int)length;
        }
        if ((ulong)position != bytes) throw new InvalidDataException("Trailing frame bytes.");
        return records;
    }

    public void Dispose()
    {
        if (_handle == 0) return;
        Native.ndb_close(_handle);
        _handle = 0;
        GC.SuppressFinalize(this);
    }

    ~NewDbmsSnapshot() => Dispose();

    private static void Check(int code)
    {
        if (code != 0) throw new IOException($"NewDBMS operation failed (status {code}).");
    }

    private static class Native
    {
        [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
        internal static extern uint ndb_abi_version();
        [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
        internal static extern int ndb_open(byte[] path, ulong length, byte[] generation, out ulong handle, out ulong count);
        [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
        internal static extern int ndb_read_page(ulong handle, ulong start, uint limit, [Out] byte[] output, ulong capacity, out ulong bytes, out ulong count);
        [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
        internal static extern int ndb_close(ulong handle);
        [DllImport("newdbms", CallingConvention = CallingConvention.Cdecl)]
        internal static extern int ndb_create(byte[] path, ulong length, byte[] generation, byte[] framed, ulong framedLength, ulong count);
    }
}
