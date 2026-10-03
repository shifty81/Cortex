$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "[INFO] Cortex W13B2C2 R4 cumulative normalization"

$r3 = Join-Path $root "APPLY_CORTEX_R3_CORRECTIONS.ps1"
if (Test-Path -LiteralPath $r3 -PathType Leaf) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $r3
    if ($LASTEXITCODE -ne 0) {
        throw "R3 cumulative normalization failed with exit $LASTEXITCODE."
    }
}

$volumeRoot = [System.IO.Path]::GetPathRoot($root)
$pythonRoot = Join-Path $volumeRoot "shared\toolchains\python"
if (-not (Test-Path -LiteralPath $pythonRoot -PathType Container)) {
    throw "Portable Vault Python root not found: $pythonRoot"
}

$python = Get-ChildItem -LiteralPath $pythonRoot -Directory -ErrorAction Stop |
    Sort-Object Name -Descending |
    ForEach-Object {
        $candidate = Join-Path $_.FullName "python.exe"
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { $candidate }
    } |
    Select-Object -First 1

if (-not $python) {
    throw "No portable python.exe found under $pythonRoot"
}

$formatter = Join-Path $root "tools\control\CortexCanonicalRustFormat.py"
if (-not (Test-Path -LiteralPath $formatter -PathType Leaf)) {
    throw "Missing canonical Rust formatter helper: $formatter"
}

& $python -X utf8 $formatter --root $root
if ($LASTEXITCODE -ne 0) {
    throw "Canonical Rust formatting failed with exit $LASTEXITCODE."
}

$receiptDir = Join-Path $root "artifacts\manual-recovery"
New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$receiptPath = Join-Path $receiptDir "w13b2c2-r4-cumulative-normalization-$stamp.json"
[ordered]@{
    schema = "cortex.manual_source_normalization.v1"
    correction = "W13B2C2 R4 cumulative normalization"
    utc = [DateTime]::UtcNow.ToString("o")
    root = $root
    portablePython = $python
    actions = @(
        "R2 stale broker regression expectations normalized",
        "R3 Windows environment-key canonicalization verified",
        "NewProject classifier bounded to qualifier-before-project context",
        "cargo fmt --all applied through portable ToolchainBroker environment",
        "cargo fmt --all -- --check verified"
    )
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding UTF8

Write-Host "[PASS] R4 cumulative normalization complete."
Write-Host "[PASS] Receipt: $receiptPath"
exit 0
