using Catdb;

static string Required(string name) => Environment.GetEnvironmentVariable(name) ?? throw new Exception(name);
static void Require(bool result, string what)
{
    if (!result) throw new Exception(what);
}

if (args is ["--check-malformed-replies"])
{
    MalformedReplies.Check();
    return;
}

var host = Required("CATDB_HOST");
int port = int.Parse(Required("CATDB_PORT"));
using var client = Environment.GetEnvironmentVariable("CATDB_TOKEN") is { Length: > 0 } token
    ? CatdbClient.ConnectToken(host, port, token)
    : CatdbClient.ConnectAccount(host, port, Required("CATDB_USER"), Required("CATDB_PASSWORD"),
        caFile: Environment.GetEnvironmentVariable("CATDB_CA_FILE"));
client.Ping();
if (Environment.GetEnvironmentVariable("CATDB_AUTH_ONLY") == "1") return;

byte[] key = [0xff, 0];
byte[] expected = Convert.FromHexString(Required("CATDB_EXPECT"));
Require(client.GetLatest(key) is { } read && read.SequenceEqual(expected), "cross-language read");
byte[] local = [0xfe, 0x63, 0x73];
client.DeleteCommit(local);
Require(client.GetLatest(local) is null, "missing key");
client.Begin();
client.Put(local, []);
Require(client.Get(local) is { Length: 0 }, "staged empty value");
client.Rollback();
Require(client.GetLatest(local) is null, "rollback");
client.Begin();
client.Put(local, []);
client.Commit();
client.BeginRead();
Require(client.Get(local) is { Length: 0 }, "committed empty value");
client.Rollback();
try
{
    client.Commit();
    throw new Exception("missing server error");
}
catch (CatdbServerException error) when (error.Code == "invalid_request") { }
client.DeleteCommit(local);
client.PutCommit(key, Convert.FromHexString(Required("CATDB_VALUE")));
Console.WriteLine("C# interoperation passed");
