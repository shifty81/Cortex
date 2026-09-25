[CmdletBinding()]
param(
    [Parameter(Position=0)]
    [string]$Root = "."
)

$ErrorActionPreference = "Stop"
$RootPath = (Resolve-Path -LiteralPath $Root).Path

$Git = Get-Command git.exe -ErrorAction SilentlyContinue
if (-not $Git) { $Git = Get-Command git -ErrorAction SilentlyContinue }
if (-not $Git) { throw "Git is required for exact PCC surface repair." }

$Commit = "d22b4f459c4fa095e42a6b9e3ae8951e170baae8"
$Rel = "tools/control/PCCSurfaceCommon.py"
$GuiRel = "tools/control/CortexPCCGui.py"

# Exact known Git object identities.
$KnownStaleBlob = "f55bfe557a9f79aabc860dfd5b9af5a2fe55567e"
$TargetBlob = "c456b1c4b9dc8724834fe00c97ad6ebd55a2ba3c"

function Get-Blob([string]$Path) {
    $v = & $Git.Source hash-object -- $Path 2>$null
    if ($LASTEXITCODE -ne 0) { return "" }
    return ([string]$v).Trim()
}

$Surface = Join-Path $RootPath $Rel
$Gui = Join-Path $RootPath $GuiRel

if (-not (Test-Path -LiteralPath $Surface -PathType Leaf)) {
    throw "Missing $Rel"
}
if (-not (Test-Path -LiteralPath $Gui -PathType Leaf)) {
    throw "Missing $GuiRel"
}

$current = Get-Blob $Surface
$guiText = Get-Content -LiteralPath $Gui -Raw -Encoding UTF8

Write-Host ""
Write-Host "CORTEX PCC SURFACE REPAIR" -ForegroundColor Cyan
Write-Host "========================="
Write-Host "Root        : $RootPath"
Write-Host "Current blob: $current"
Write-Host "Stale blob  : $KnownStaleBlob"
Write-Host "Target blob : $TargetBlob"
Write-Host ""

if ($current -eq $TargetBlob) {
    Write-Host "[PASS] PCCSurfaceCommon.py already matches the certified d22 object." -ForegroundColor Green
}
elseif ($current -eq $KnownStaleBlob) {
    if (-not $guiText.Contains("SurfaceCommand")) {
        throw "Refusing repair: GUI does not import SurfaceCommand, so this is not the known mixed-generation failure."
    }

    & $Git.Source -C $RootPath cat-file -e "$Commit^{commit}" 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "Exact source commit $Commit is not available in local Git history. No network fetch is attempted."
    }

    $temp = Join-Path $env:TEMP ("PCCSurfaceCommon-" + [guid]::NewGuid().ToString("N") + ".py")
    try {
        $content = & $Git.Source -C $RootPath show "$Commit`:$Rel"
        if ($LASTEXITCODE -ne 0) { throw "Unable to extract $Rel from $Commit" }

        [IO.File]::WriteAllText(
            $temp,
            (($content -join "`n") + "`n"),
            [Text.UTF8Encoding]::new($false)
        )

        $extracted = Get-Blob $temp
        if ($extracted -ne $TargetBlob) {
            throw "Extracted object verification failed. Expected $TargetBlob, got $extracted"
        }

        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $backupDir = Join-Path $RootPath "artifacts/repair-backups/PCC-SURFACE-$stamp"
        New-Item -ItemType Directory -Force -Path $backupDir | Out-Null
        $backup = Join-Path $backupDir "PCCSurfaceCommon.py.before"
        Copy-Item -LiteralPath $Surface -Destination $backup -Force

        $swap = "$Surface.swap-$([guid]::NewGuid().ToString('N'))"
        Copy-Item -LiteralPath $temp -Destination $swap -Force
        Move-Item -LiteralPath $swap -Destination $Surface -Force

        $after = Get-Blob $Surface
        if ($after -ne $TargetBlob) {
            Copy-Item -LiteralPath $backup -Destination $Surface -Force
            throw "Post-repair object verification failed. Original was restored."
        }

        $receipt = [ordered]@{
            schema = "cortex.pcc_surface_repair.v1"
            createdLocal = (Get-Date).ToString("o")
            projectRoot = $RootPath
            sourceCommit = $Commit
            path = $Rel
            beforeBlob = $current
            afterBlob = $after
            backup = $backup
            status = "APPLIED_UNVERIFIED"
        }
        $receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $backupDir "receipt.json") -Encoding UTF8

        Write-Host "[PASS] Replaced stale PCCSurfaceCommon.py with the exact matching d22 object." -ForegroundColor Green
        Write-Host "Backup      : $backup"
        Write-Host "Receipt     : $(Join-Path $backupDir 'receipt.json')"
    }
    finally {
        Remove-Item -LiteralPath $temp -Force -ErrorAction SilentlyContinue
    }
}
else {
    throw @"
Refusing automatic repair.
PCCSurfaceCommon.py does not match either known object:
  stale  = $KnownStaleBlob
  target = $TargetBlob

This means the live file is a different/newer revision. It must be preserved and diffed rather than overwritten.
"@
}

# Import-only smoke: no registry write, no Vault operation, no build.
$Python = Get-Command python.exe -ErrorAction SilentlyContinue
if (-not $Python) { $Python = Get-Command python -ErrorAction SilentlyContinue }
if (-not $Python) {
    Write-Host "[WARN] Python is not on PATH; source object repair is complete but import smoke was skipped." -ForegroundColor Yellow
    exit 0
}

$oldDontWrite = $env:PYTHONDONTWRITEBYTECODE
$oldPyPath = $env:PYTHONPATH
try {
    $env:PYTHONDONTWRITEBYTECODE = "1"
    $controlDir = Join-Path $RootPath "tools/control"
    $env:PYTHONPATH = if ($oldPyPath) { "$controlDir;$oldPyPath" } else { $controlDir }
    & $Python.Source -c "from PCCSurfaceCommon import SurfaceCommand, command_catalog; print('PASS PCCSurfaceCommon import contract')"
    if ($LASTEXITCODE -ne 0) { throw "PCCSurfaceCommon import smoke failed after repair." }
}
finally {
    $env:PYTHONDONTWRITEBYTECODE = $oldDontWrite
    $env:PYTHONPATH = $oldPyPath
}

Write-Host ""
Write-Host "[PASS] PCC surface import contract is repaired." -ForegroundColor Green
Write-Host "Next: launch PROJECT_CONTROL_CENTER.cmd normally."
Write-Host "If the GUI opens, do NOT call the source GREEN yet; run the normal FULL gate afterward."
