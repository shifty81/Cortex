$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "[INFO] Cortex W13B2C2 R5 cumulative normalization"

$r4 = Join-Path $root "APPLY_CORTEX_R4_CORRECTIONS.ps1"
if (Test-Path -LiteralPath $r4 -PathType Leaf) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $r4
    if ($LASTEXITCODE -ne 0) {
        throw "R4 cumulative normalization failed with exit $LASTEXITCODE."
    }
}

$test = Join-Path $root "tools\control\tests\test_w13b2c2_rollup_corrections.py"
if (-not (Test-Path -LiteralPath $test -PathType Leaf)) {
    throw "Missing R5 regression source: $test"
}

$source = [System.IO.File]::ReadAllText($test)
if ($source.Contains('self.assertIn(''"standalone",'', source)')) {
    throw "R5 verification failed: stale comma-delimited standalone assertion remains."
}
if (-not $source.Contains('self.assertIn(''"standalone"'', source)')) {
    throw "R5 verification failed: normalized standalone classifier assertion is missing."
}
if (-not $source.Contains('self.assertIn("let qualified_project = words.iter().enumerate()", source)')) {
    throw "R5 verification failed: bounded NewProject classifier assertion is missing."
}

$receiptDir = Join-Path $root "artifacts\manual-recovery"
New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$receiptPath = Join-Path $receiptDir "w13b2c2-r5-regression-normalization-$stamp.json"
[ordered]@{
    schema = "cortex.manual_source_normalization.v1"
    correction = "W13B2C2 R5 stale rollup regression source-shape assertion"
    utc = [DateTime]::UtcNow.ToString("o")
    root = $root
    verified = @(
        "bounded NewProject classifier remains present",
        "old comma-delimited standalone qualifier assertion removed",
        "rustfmt-stable standalone token assertion present"
    )
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding UTF8

Write-Host "[PASS] R5 rollup regression normalization verified."
Write-Host "[PASS] Receipt: $receiptPath"
exit 0
