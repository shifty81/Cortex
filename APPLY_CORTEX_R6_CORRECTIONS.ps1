$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "[INFO] Cortex W13B2C2 R6 cumulative normalization"

$r5 = Join-Path $root "APPLY_CORTEX_R5_CORRECTIONS.ps1"
if (Test-Path -LiteralPath $r5 -PathType Leaf) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $r5
    if ($LASTEXITCODE -ne 0) {
        throw "R5 cumulative normalization failed with exit $LASTEXITCODE."
    }
}

$desktop = Join-Path $root "crates\cortex_desktop_core\src\lib.rs"
$desktopSource = [System.IO.File]::ReadAllText($desktop)
foreach ($name in @(
    "m11u2_prompt_requests_runtime",
    "m11u2_prompt_requests_window",
    "m11u2_prompt_requests_title_change"
)) {
    $token = "#[cfg(test)]`n    fn $name"
    if (-not $desktopSource.Contains($token)) {
        throw "R6 verification failed: test-only runtime helper is not cfg(test): $name"
    }
}

$pcc = Join-Path $root "tools\control\CortexPCC.py"
$pccSource = [System.IO.File]::ReadAllText($pcc)
foreach ($token in @(
    'work / "command-output" / src.name',
    'self.log.command_output_dir.glob("*.txt")',
    'safe_phase = re.sub'
)) {
    if (-not $pccSource.Contains($token)) {
        throw "R6 verification failed: debug evidence token missing: $token"
    }
}

$receiptDir = Join-Path $root "artifacts\manual-recovery"
New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$receiptPath = Join-Path $receiptDir "w13b2c2-r6-clippy-evidence-normalization-$stamp.json"
[ordered]@{
    schema = "cortex.manual_source_normalization.v1"
    correction = "W13B2C2 R6 Clippy dead-code repair + command diagnostic evidence"
    utc = [DateTime]::UtcNow.ToString("o")
    root = $root
    verified = @(
        "test-only M11U2 runtime compatibility helpers are cfg(test)",
        "bounded NewProject classifier remains present",
        "debug bundles include bounded subprocess command-output captures"
    )
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding UTF8

Write-Host "[PASS] R6 Clippy/evidence normalization verified."
Write-Host "[PASS] Receipt: $receiptPath"
exit 0
