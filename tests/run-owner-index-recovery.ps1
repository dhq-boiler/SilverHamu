# Run under PowerShell 7 (.NET 10) after building Catdb.Owner.Sql.Tests for the selected configuration.
param([string] $WriterDirectory, [ValidateSet('Debug','Release')][string] $Configuration = 'Debug')
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$output = Join-Path $root "apps/Catdb.Owner.Sql.Tests/bin/$Configuration/net10.0"
$env:PATH = "$output;$env:PATH"
Add-Type -Path (Join-Path $output 'Catdb.Sql.dll')

function Open-TestDatabase($directory, [bool] $create) {
    $path = Join-Path $directory 'index.ndb'
    return [Catdb.Sql.CatdbSqlConnection]::OpenAsync($path, $create).GetAwaiter().GetResult()
}

if ($WriterDirectory) {
    $sql = Open-TestDatabase $WriterDirectory $true
    $workload = @'
CREATE TABLE t(id INT PRIMARY KEY, shelf TEXT);
INSERT INTO t VALUES (1,'A'),(2,'removed'),(5,'B');
CREATE INDEX by_shelf ON t(shelf);
UPDATE t SET id=3, shelf='B' WHERE id=1;
DELETE FROM t WHERE id=2;
INSERT INTO t VALUES (6,'B');
BEGIN;
UPDATE t SET id=4, shelf='pending' WHERE id=3;
DELETE FROM t WHERE id=5;
'@
    $sql.ExecuteAsync($workload).GetAwaiter().GetResult() | Out-Null
    [IO.File]::WriteAllText((Join-Path $WriterDirectory 'ready'), '3')
    [Diagnostics.Process]::GetCurrentProcess().Kill()
    return
}

$directory = (New-Item -ItemType Directory -Path (Join-Path ([IO.Path]::GetTempPath()) ("catdb-index-recovery-" + [guid]::NewGuid()))).FullName
& (Get-Process -Id $PID).Path -NoProfile -File $PSCommandPath -WriterDirectory $directory -Configuration $Configuration
if ($LASTEXITCODE -eq 0 -or !(Test-Path (Join-Path $directory 'ready'))) { throw 'Writer did not reach the crash point.' }
$sql = Open-TestDatabase $directory $false
try {
    foreach ($id in 1..4) {
        $queries = "SELECT id FROM t WHERE id=$id", "SELECT id FROM t WHERE id=$id AND TRUE"
        foreach ($query in $queries) {
            $rows = $sql.ExecuteAsync($query).GetAwaiter().GetResult()[0].Rows
            $expected = [int]($id -eq 3)
            if ($rows.Count -ne $expected -or ($expected -and $rows[0][0].Display -ne '3')) {
                throw "Unexpected result after process termination: $query"
            }
        }
    }
    $expectedShelves = @{ A = ''; B = '3,5,6'; removed = ''; pending = '' }
    foreach ($shelf in $expectedShelves.Keys) {
        foreach ($condition in "shelf='$shelf'", "shelf='$shelf' AND TRUE") {
            $rows = $sql.ExecuteAsync("SELECT id FROM t WHERE $condition ORDER BY id").GetAwaiter().GetResult()[0].Rows
            $ids = ($rows | ForEach-Object { $_[0].Display }) -join ','
            if ($ids -ne $expectedShelves[$shelf]) {
                throw "Unexpected recovered IDs '$ids' for $condition"
            }
        }
    }
    'Recovered primary and secondary index results agree with scans and committed rows.'
} finally { $sql.Dispose() }
