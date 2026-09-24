param(
    [string]$Root = "E:\Cortex",
    [string]$OutDir = "E:\"
)

$ErrorActionPreference = "Stop"

$files = @(
    "config\cortex\github_authority.v1.json",
    "crates\cortex_desktop_core\src\lib.rs",
    "crates\cortex_registry\src\lib.rs",
    "crates\cortex_workspace\src\lib.rs",
    "scripts\Bootstrap-CortexEnvironment.ps1",
    "tools\control\CortexGitAuthority.py",
    "tools\control\CortexPCC.py",
    "tools\control\CortexPCCGui.py",
    "tools\control\PCCStoragePaths.py",
    "tools\control\PCCSurfaceCommon.py",
    "tools\control\PCCVaultStorage.py",
    "tools\control\tests\test_cortex_pcc.py",
    "tools\control\tests\test_pcc_vault_storage.py"
)

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$stage = Join-Path $env:TEMP "Cortex_R17D_Preimage_$stamp"
$payload = Join-Path $stage "source"
New-Item -ItemType Directory -Force -Path $payload | Out-Null

$records = @()

foreach ($rel in $files) {
    $src = Join-Path $Root $rel
    $exists = Test-Path -LiteralPath $src -PathType Leaf

    $record = [ordered]@{
        path   = $rel.Replace("\", "/")
        exists = $exists
        bytes  = $null
        sha256 = $null
    }

    if ($exists) {
        $hash = Get-FileHash -LiteralPath $src -Algorithm SHA256
        $info = Get-Item -LiteralPath $src
        $record.bytes = $info.Length
        $record.sha256 = $hash.Hash.ToLowerInvariant()

        $dst = Join-Path $payload $rel
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
        Copy-Item -LiteralPath $src -Destination $dst -Force
    }

    $records += [pscustomobject]$record
}

$meta = [ordered]@{
    schema = "cortex.r17d.preimage.snapshot.v1"
    captured_at = (Get-Date).ToString("o")
    root = $Root
    files = $records
}

$meta | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $stage "PREIMAGE_HASHES.json") -Encoding UTF8

$gitStatus = Join-Path $stage "GIT_STATUS.txt"
try {
    & git -C $Root status --short --branch 2>&1 | Set-Content -LiteralPath $gitStatus -Encoding UTF8
} catch {
    "git status unavailable: $($_.Exception.Message)" | Set-Content -LiteralPath $gitStatus -Encoding UTF8
}

$out = Join-Path $OutDir "Cortex_R17D_PREIMAGE_$stamp.zip"
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $out -Force
Remove-Item -LiteralPath $stage -Recurse -Force

Write-Host ""
Write-Host "R17D exact preimage snapshot created:"
Write-Host $out
Write-Host ""
Write-Host "Upload that ZIP to ChatGPT. No Cortex source files were modified."
