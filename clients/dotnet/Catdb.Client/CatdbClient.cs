using System.Buffers.Binary;
using System.Globalization;
using System.Net.Security;
using System.Net.Sockets;
using System.Security.Authentication;
using System.Security.Cryptography.X509Certificates;
using System.Text;
using System.Text.Json;

namespace Catdb;

public sealed class CatdbServerException(string code) : Exception($"catdb: {code}")
{
    public string Code { get; } = code;
}

/// <summary>One ordered service connection. Use a separate client for each concurrent transaction.</summary>
public sealed class CatdbClient : IDisposable
{
    private const int MaxRequest = 1024 * 1024;
    private const int MaxResponse = 2 * 1024 * 1024 + 8192;
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);
    private readonly object gate = new();
    private readonly TcpClient socket;
    private readonly Stream stream;
    private readonly TimeSpan timeout;
    private bool closed;

    private CatdbClient(TcpClient socket, Stream stream, TimeSpan timeout)
    {
        this.socket = socket;
        this.stream = stream;
        this.timeout = timeout;
    }

    public static CatdbClient ConnectToken(string host, int port, string token, TimeSpan? timeout = null) =>
        Connect(host, port, new { op = "auth", version = 1, token }, 1, null, timeout);

    public static CatdbClient ConnectAccount(string host, int port, string username, string password,
        string database = "main", string? caFile = null, TimeSpan? timeout = null) =>
        Connect(host, port, new { op = "auth", version = 2, username, password, database }, 2, caFile, timeout);

    private static CatdbClient Connect(string host, int port, object auth, int protocol, string? caFile, TimeSpan? limit)
    {
        var timeout = limit ?? TimeSpan.FromSeconds(10);
        if (timeout <= TimeSpan.Zero)
            throw new ArgumentOutOfRangeException(nameof(limit));
        using var deadline = new CancellationTokenSource(timeout);
        var socket = new TcpClient { NoDelay = true };
        Stream? stream = null;
        var certificates = new X509Certificate2Collection();
        try
        {
            socket.ConnectAsync(host, port, deadline.Token).AsTask().GetAwaiter().GetResult();
            stream = socket.GetStream();
            if (protocol == 2)
            {
                var options = new SslClientAuthenticationOptions
                {
                    TargetHost = host,
                    EnabledSslProtocols = SslProtocols.Tls13
                };
                if (caFile is not null)
                {
                    certificates.ImportFromPem(File.ReadAllText(caFile));
                    if (certificates.Count == 0)
                        throw new InvalidDataException("No certificates in CA file");
                    options.CertificateChainPolicy = new X509ChainPolicy
                    {
                        TrustMode = X509ChainTrustMode.CustomRootTrust,
                        RevocationMode = X509RevocationMode.NoCheck
                    };
                    options.CertificateChainPolicy.CustomTrustStore.AddRange(certificates);
                }
                var tls = new SslStream(stream);
                stream = tls;
                tls.AuthenticateAsClientAsync(options, deadline.Token).GetAwaiter().GetResult();
            }
            var client = new CatdbClient(socket, stream, timeout);
            var response = client.Request(auth);
            if (response.GetProperty("protocol").GetInt32() != protocol)
                throw new InvalidDataException("Unexpected catdb protocol version");
            return client;
        }
        catch
        {
            stream?.Dispose();
            socket.Dispose();
            throw;
        }
        finally
        {
            foreach (var certificate in certificates)
                certificate.Dispose();
        }
    }

    /// <summary>Send a wire request, including operations such as scan. Server errors retain their code.</summary>
    public JsonElement Request(object request) => Execute(request, response => response);

    private T Execute<T>(object request, Func<JsonElement, T> decode)
    {
        byte[] payload = JsonSerializer.SerializeToUtf8Bytes(request);
        if (payload.Length > MaxRequest)
            throw new ArgumentException("catdb request exceeds 1 MiB", nameof(request));
        lock (gate)
        {
            ObjectDisposedException.ThrowIf(closed, this);
            using var deadline = new CancellationTokenSource(timeout);
            try
            {
                var response = ExchangeAsync(payload, deadline.Token).GetAwaiter().GetResult();
                if (!response.GetProperty("ok").GetBoolean())
                {
                    var code = response.GetProperty("error").GetString();
                    if (string.IsNullOrEmpty(code))
                        throw new InvalidDataException("Invalid catdb error response");
                    throw new CatdbServerException(code);
                }
                return decode(response);
            }
            catch (CatdbServerException)
            {
                throw;
            }
            catch (Exception error)
            {
                Dispose();
                throw new IOException("catdb exchange failed; a submitted write may have committed", error);
            }
        }
    }

    private async Task<JsonElement> ExchangeAsync(byte[] payload, CancellationToken cancellation)
    {
        var frame = new byte[sizeof(int) + payload.Length];
        BinaryPrimitives.WriteInt32BigEndian(frame, payload.Length);
        payload.CopyTo(frame, sizeof(int));
        await stream.WriteAsync(frame, cancellation).ConfigureAwait(false);
        var header = new byte[sizeof(int)];
        await stream.ReadExactlyAsync(header, cancellation).ConfigureAwait(false);
        int length = BinaryPrimitives.ReadInt32BigEndian(header);
        if (length <= 0 || length > MaxResponse)
            throw new InvalidDataException("Invalid catdb response size");
        var body = new byte[length];
        await stream.ReadExactlyAsync(body, cancellation).ConfigureAwait(false);
        StrictUtf8.GetCharCount(body);
        return JsonSerializer.Deserialize<JsonElement>(body);
    }

    private ulong Version(object request) =>
        Execute(request, response => ulong.Parse(response.GetProperty("version").GetString()!,
            NumberStyles.None, CultureInfo.InvariantCulture));

    private byte[]? ReadValue(string op, ReadOnlySpan<byte> key)
    {
        return Execute(new { op, key = Convert.ToHexString(key) }, response =>
        {
            var value = response.GetProperty("value").GetString();
            return value is null ? null : Convert.FromHexString(value);
        });
    }

    public ulong Ping() => Version(new { op = "ping" });
    public byte[]? GetLatest(ReadOnlySpan<byte> key) => ReadValue("get_latest", key);
    public ulong PutCommit(ReadOnlySpan<byte> key, ReadOnlySpan<byte> value) =>
        Version(new { op = "put_commit", key = Convert.ToHexString(key), value = Convert.ToHexString(value) });
    public ulong DeleteCommit(ReadOnlySpan<byte> key) =>
        Version(new { op = "delete_commit", key = Convert.ToHexString(key) });
    public ulong Begin() => Version(new { op = "begin" });
    public ulong BeginRead() => Version(new { op = "begin_read" });
    public byte[]? Get(ReadOnlySpan<byte> key) => ReadValue("get", key);
    public void Put(ReadOnlySpan<byte> key, ReadOnlySpan<byte> value) =>
        Request(new { op = "put", key = Convert.ToHexString(key), value = Convert.ToHexString(value) });
    public void Delete(ReadOnlySpan<byte> key) => Request(new { op = "delete", key = Convert.ToHexString(key) });
    public ulong Commit() => Version(new { op = "commit" });
    public void Rollback() => Request(new { op = "rollback" });

    public void Dispose()
    {
        lock (gate)
        {
            if (closed) return;
            closed = true;
            try { stream.Dispose(); }
            finally { socket.Dispose(); }
        }
    }
}
