$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$old = 'BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.4"'
$new = 'BROKER_VERSION = "PCC-TOOLCHAIN-BROKER-0.5"'

$targets = @(
    "tools\control\tests\test_cortex_pcc.py",
    "tools\control\tests\test_pcc_failure_evidence_performance.py",
    "tools\control\tests\test_pcc_runtime_recovery_r8.py"
)

$states = @()
foreach ($rel in $targets) {
    $path = Join-Path $root $rel
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Required regression file is missing: $rel"
    }

    $text = [System.IO.File]::ReadAllText($path)
    $oldCount = ([regex]::Matches($text, [regex]::Escape($old))).Count
    $newCount = ([regex]::Matches($text, [regex]::Escape($new))).Count

    if ($oldCount -eq 1 -and $newCount -eq 0) {
        $states += [pscustomobject]@{ Relative=$rel; Path=$path; Action="replace" }
    }
    elseif ($oldCount -eq 0 -and $newCount -eq 1) {
        $states += [pscustomobject]@{ Relative=$rel; Path=$path; Action="already" }
    }
    else {
        throw "Unexpected broker-version test state in $rel (old=$oldCount new=$newCount). Refusing partial mutation."
    }
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$backupRoot = Join-Path $root "artifacts\manual-recovery\w13b2c2-r2-stale-broker-tests-$stamp"
$changed = @()

foreach ($state in $states | Where-Object { $_.Action -eq "replace" }) {
    $backup = Join-Path $backupRoot $state.Relative
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $backup) | Out-Null
    Copy-Item -LiteralPath $state.Path -Destination $backup -Force

    $text = [System.IO.File]::ReadAllText($state.Path)
    $updated = $text.Replace($old, $new)
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($state.Path, $updated, $utf8NoBom)
    $changed += $state.Relative
}

foreach ($rel in $targets) {
    $path = Join-Path $root $rel
    $text = [System.IO.File]::ReadAllText($path)
    $oldCount = ([regex]::Matches($text, [regex]::Escape($old))).Count
    $newCount = ([regex]::Matches($text, [regex]::Escape($new))).Count
    if ($oldCount -ne 0 -or $newCount -ne 1) {
        throw "Post-write verification failed for $rel (old=$oldCount new=$newCount)."
    }
}

$receiptDir = Join-Path $root "artifacts\manual-recovery"
New-Item -ItemType Directory -Force -Path $receiptDir | Out-Null
$receipt = [ordered]@{
    schema = "cortex.manual_source_normalization.v1"
    correction = "W13B2C2 R2 stale ToolchainBroker regression expectations"
    utc = [DateTime]::UtcNow.ToString("o")
    oldVersion = "PCC-TOOLCHAIN-BROKER-0.4"
    newVersion = "PCC-TOOLCHAIN-BROKER-0.5"
    changed = $changed
    backupRoot = if ($changed.Count) { $backupRoot } else { $null }
}
$receiptPath = Join-Path $receiptDir "w13b2c2-r2-test-normalization-$stamp.json"
$receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding UTF8

Write-Host "[PASS] Stale ToolchainBroker regression expectations are normalized to 0.5."
if ($changed.Count) {
    Write-Host "[PASS] Updated: $($changed -join ', ')"
    Write-Host "[PASS] Backup: $backupRoot"
}
else {
    Write-Host "[PASS] All three regression files were already normalized."
}
Write-Host "[PASS] Receipt: $receiptPath"
exit 0
