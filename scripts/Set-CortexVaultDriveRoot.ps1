[CmdletBinding(SupportsShouldProcess=$true)]
param(
    [string]$DriveRoot,
    [switch]$CurrentProcessOnly
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($DriveRoot)) {
    $DriveRoot = [System.IO.Path]::GetPathRoot((Resolve-Path -LiteralPath $repoRoot).Path)
}
if ([string]::IsNullOrWhiteSpace($DriveRoot)) {
    throw 'Unable to determine the drive root. Pass -DriveRoot E:\ explicitly.'
}

$full = [System.IO.Path]::GetFullPath($DriveRoot)
$root = [System.IO.Path]::GetPathRoot($full)
if ([string]::IsNullOrWhiteSpace($root) -or ($full.TrimEnd('\\') -ne $root.TrimEnd('\\'))) {
    throw "Vault location must be the root of a drive (for example E:\). Received: $DriveRoot"
}
if (-not (Test-Path -LiteralPath $root -PathType Container)) {
    throw "Drive root does not exist: $root"
}

$probe = Join-Path $root ('.cortex-write-probe-' + [guid]::NewGuid().ToString('N') + '.tmp')
try {
    [System.IO.File]::WriteAllText($probe, 'probe')
} finally {
    Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
}

if ($PSCmdlet.ShouldProcess($root, 'Configure Cortex Vault authority at drive root')) {
    $env:CORTEX_VAULT_ROOT = $root
    if (-not $CurrentProcessOnly) {
        [Environment]::SetEnvironmentVariable('CORTEX_VAULT_ROOT', $root, 'User')
    }

    foreach ($relative in @('projects', 'objects\sha256', 'shared', 'quarantine')) {
        New-Item -ItemType Directory -Path (Join-Path $root $relative) -Force | Out-Null
    }

    $marker = [ordered]@{
        schema = 'cortex.vault_drive_root.v1'
        vaultRoot = $root
        configuredAt = [DateTimeOffset]::UtcNow.ToString('o')
        configuredBy = 'Set-CortexVaultDriveRoot.ps1'
        repository = (Resolve-Path -LiteralPath $repoRoot).Path
        note = 'The entire drive root is the Cortex Vault authority. Cortex only owns its governed namespaces; unrelated drive contents are not automatically moved or deleted.'
    }
    $markerPath = Join-Path $root '.cortex-vault-root.json'
    $marker | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $markerPath -Encoding UTF8

    Write-Host "Cortex Vault root configured: $root"
    Write-Host "User environment: CORTEX_VAULT_ROOT=$root"
    Write-Host "Marker: $markerPath"
    Write-Host 'Restart the PCC/Forge GUI so all child processes inherit the new Vault root.'
}
