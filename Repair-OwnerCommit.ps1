[CmdletBinding()]
param([switch]$CheckOnly)
$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$original = '3a99a6c6326ba4e502223225c7fff813960f6be4'
$parent = '07ba7af92839d29e92714013d2783cfdb1af940c'
function Invoke-Git {
    & git -C $repo @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args[0]) failed ($LASTEXITCODE)." }
}
$previousEncoding = [Console]::OutputEncoding
$temp = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString())
$encoding = [Text.UTF8Encoding]::new($false)
try {
    [Console]::OutputEncoding = $encoding
    if ((Invoke-Git symbolic-ref --short HEAD) -ne 'main' -or
        (Invoke-Git rev-parse HEAD) -ne $original -or
        (Invoke-Git rev-parse refs/remotes/origin/main) -ne $parent) {
        throw 'The branch has changed. Review it before amending.'
    }
    if (Invoke-Git diff --cached --name-only) { throw 'There are already staged changes.' }
    foreach ($marker in 'MERGE_HEAD','CHERRY_PICK_HEAD','REVERT_HEAD','rebase-merge','rebase-apply','sequencer') {
        if (Test-Path -LiteralPath (Invoke-Git rev-parse --path-format=absolute --git-path $marker)) { throw "Git operation in progress: $marker" }
    }
    [void][IO.Directory]::CreateDirectory($temp)
    $page = 'apps/Catdb.Owner/MainPage.cs'
    $picker = 'apps/Catdb.Owner/Services/DatabaseFilePicker.cs'
    $readme = 'apps/Catdb.Owner/README.md'
    $old = @(Invoke-Git show "${parent}:$page")
    $lines = [Collections.Generic.List[string]]::new()
    $lines.AddRange([string[]]@(Invoke-Git show "${original}:$page"))
    # Splice the original TLS and backup behavior into the committed Owner UI.
    # Coordinates are tied to the two full object IDs checked above, in descending order.
    function Restore-Lines([int]$at, [int]$remove, [int]$first, [int]$last) {
        $lines.RemoveRange($at - 1,$remove)
        $lines.InsertRange($at - 1,[string[]]$old[($first - 1)..($last - 1)])
    }
    Restore-Lines 570 0 579 579
    Restore-Lines 357 0 380 380
    Restore-Lines 329 0 362 362
    Restore-Lines 306 0 320 326
    $lines.RemoveRange(284,2)
    Restore-Lines 273 1 294 294
    Restore-Lines 262 1 283 283
    Restore-Lines 211 1 247 249
    Restore-Lines 207 1 243 243
    Restore-Lines 202 1 234 238
    Restore-Lines 200 1 229 232
    Restore-Lines 193 1 222 222
    Restore-Lines 186 2 201 216
    Restore-Lines 175 2 184 191
    Restore-Lines 49 0 53 53
    Restore-Lines 32 0 32 34

    $doc = ((Invoke-Git show "${parent}:$readme") -join "`n").Replace('Open a local catdb file directly, connect to a service, or explore the in-memory demo.', 'Open a local catdb file directly or connect to a service.')
    $doc = $doc.Replace('## Service connections', "Open a .catlink file to browse the tables in its target folder. Select a table to view its rows, or run a read-only SELECT query in the SQL editor. For example:`n`n~~~~sql`nSELECT * FROM Book_3 WHERE PublishDate IS NOT NULL;`n~~~~`n`n## Service connections")
    $filter = ((Invoke-Git show "${original}:$picker") -join "`n").Replace('.backup','.catbak')
    $desired = [ordered]@{}
    $desired[$page] = $lines -join "`n"
    $desired[$picker] = $filter
    $desired[$readme] = $doc
    $blobs = @{}
    foreach ($path in $desired.Keys) {
        $file = Join-Path $temp ([IO.Path]::GetFileName($path))
        [IO.File]::WriteAllText($file, $desired[$path] + "`n", $encoding)
        $id = Invoke-Git hash-object -- $file
        $blobs[$path] = $id
        Write-Output "$path  $id"
    }
    if ($CheckOnly) { return }
    foreach ($path in $desired.Keys) {
        $file = Join-Path $temp ([IO.Path]::GetFileName($path))
        $null = Invoke-Git hash-object -w -- $file
    }
    if ((Invoke-Git rev-parse HEAD) -ne $original -or (Invoke-Git diff --cached --name-only)) { throw 'HEAD or index changed.' }
    $entries = @()
    foreach ($path in $desired.Keys) { $entries += @('--cacheinfo',"100644,$($blobs[$path]),$path") }
    Invoke-Git update-index @entries
    Invoke-Git diff --cached --check
    Invoke-Git commit --amend --no-edit
    if ((Invoke-Git rev-parse 'HEAD^') -ne $parent) { throw 'Unexpected parent.' }
    foreach ($path in $desired.Keys) {
        if ((Invoke-Git rev-parse "HEAD:$path") -ne $blobs[$path]) { throw "Unexpected committed content: $path" }
    }
    Invoke-Git log -1 --oneline
    Invoke-Git status --short --branch
} finally {
    [Console]::OutputEncoding = $previousEncoding
    if (Test-Path -LiteralPath $temp) {
        foreach ($file in 'MainPage.cs','DatabaseFilePicker.cs','README.md') {
            $path = Join-Path $temp $file
            if (Test-Path -LiteralPath $path) { [IO.File]::Delete($path) }
        }
        [IO.Directory]::Delete($temp)
    }
}
