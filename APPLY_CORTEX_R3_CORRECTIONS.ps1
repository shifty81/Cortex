$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Write-Host "[INFO] Cortex W13B2C2 R3 cumulative normalization"

# 1) Normalize the stale regression contracts from R2 if needed.
$r2 = Join-Path $root "APPLY_CORTEX_R2_TEST_CONTRACT_FIX.ps1"
if (Test-Path -LiteralPath $r2 -PathType Leaf) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $r2
    if ($LASTEXITCODE -ne 0) {
        throw "R2 regression-contract normalization failed with exit $LASTEXITCODE."
    }
}

# 2) Verify the ToolchainBroker 0.5 environment-merge correction now present in source.
$shared = Join-Path $root "tools\control\PCCSharedEnvironment.py"
if (-not (Test-Path -LiteralPath $shared -PathType Leaf)) {
    throw "Missing ToolchainBroker source: $shared"
}
$source = [System.IO.File]::ReadAllText($shared)

$required = @(
    'BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.5"',
    'canonical = "PATH" if folded == "path" else key',
    '_merge_environment_overlay(result, captured)'
)
foreach ($token in $required) {
    if (-not $source.Contains($token)) {
        throw "R3 ToolchainBroker verification failed; missing source token: $token"
    }
}
if ($source.Contains('matches[0] if matches else key')) {
    throw "R3 ToolchainBroker verification failed; stale case-preserving canonicalization remains."
}

# 3) Verify shared dependency policy still publishes the exact npm/PIP key spelling consumers expect.
$vaultStorage = Join-Path $root "tools\control\PCCVaultStorage.py"
$vaultSource = [System.IO.File]::ReadAllText($vaultStorage)
foreach ($token in @(
    '"npm_config_cache": str(paths["npm_cache"])',
    '"PIP_CACHE_DIR": str(paths["pip_cache"])'
)) {
    if (-not $vaultSource.Contains($token)) {
        throw "R3 shared dependency verification failed; missing source token: $token"
    }
}

$receiptDir = Join-Path $root "artifacts\manual-recovery"
New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$receiptPath = Join-Path $receiptDir "w13b2c2-r3-environment-key-normalization-$stamp.json"
[ordered]@{
    schema = "cortex.manual_source_normalization.v1"
    correction = "W13B2C2 R3 Windows environment-key canonicalization"
    utc = [DateTime]::UtcNow.ToString("o")
    brokerVersion = "PCC-TOOLCHAIN-BROKER-0.5"
    invariant = "PATH is canonical uppercase; all other overlay keys retain authoritative overlay spelling after case-insensitive duplicate removal"
    verified = @(
        "tools/control/PCCSharedEnvironment.py",
        "tools/control/PCCVaultStorage.py"
    )
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding UTF8

Write-Host "[PASS] R3 ToolchainBroker environment-key normalization verified."
Write-Host "[PASS] npm_config_cache and PIP_CACHE_DIR authoritative spellings verified."
Write-Host "[PASS] Receipt: $receiptPath"
exit 0
