[CmdletBinding(SupportsShouldProcess=$true)]
param()
$ErrorActionPreference = 'Stop'
$current = [Environment]::GetEnvironmentVariable('CORTEX_VAULT_ROOT', 'User')
$display = if ([string]::IsNullOrWhiteSpace($current)) { '<unset>' } else { $current }
if ($PSCmdlet.ShouldProcess($display, 'Clear the per-user Cortex Vault root override')) {
    [Environment]::SetEnvironmentVariable('CORTEX_VAULT_ROOT', $null, 'User')
    Remove-Item Env:CORTEX_VAULT_ROOT -ErrorAction SilentlyContinue
    Write-Host 'Cleared CORTEX_VAULT_ROOT user override. Cortex will return to the project/default Vault policy on next launch.'
}
