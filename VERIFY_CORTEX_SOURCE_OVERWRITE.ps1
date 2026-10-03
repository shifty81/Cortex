$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$manifest = Join-Path $here "SOURCE_OVERWRITE_MANIFEST.json"
if (-not (Test-Path -LiteralPath $manifest)) { throw "Missing SOURCE_OVERWRITE_MANIFEST.json" }
$data = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$bad = @()
foreach ($f in $data.files) {
    $p = Join-Path $here $f.path
    if (-not (Test-Path -LiteralPath $p -PathType Leaf)) { $bad += "MISSING $($f.path)"; continue }
    $h = (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLowerInvariant()
    $len = (Get-Item -LiteralPath $p).Length
    if ($h -ne $f.sha256 -or $len -ne [int64]$f.bytes) { $bad += "MISMATCH $($f.path)" }
}
if ($bad.Count) { $bad | ForEach-Object { Write-Host "[FAIL] $_" }; exit 1 }
Write-Host "[PASS] Cortex cumulative source overwrite files match manifest: $($data.files.Count) files."
exit 0
