[CmdletBinding()]
param(
    [ValidateSet('Startup','Check','Hydrate','Full')]
    [string]$Mode = 'Startup',
    [string]$VaultRoot,
    [switch]$NoNetwork,
    [switch]$IncludeBuildTools
)

$ErrorActionPreference = 'Stop'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$RepoRoot = (Resolve-Path -LiteralPath (Split-Path -Parent $PSScriptRoot)).Path
$Artifacts = Join-Path $RepoRoot 'artifacts\bootstrap'
$StateDir = Join-Path $RepoRoot '.cortex'
New-Item -ItemType Directory -Path $Artifacts -Force | Out-Null
New-Item -ItemType Directory -Path $StateDir -Force | Out-Null
$HealthPath = Join-Path $Artifacts 'environment-health.json'
$EnvCmdPath = Join-Path $StateDir 'bootstrap-env.cmd'
$LogPath = Join-Path $Artifacts 'bootstrap.log'

function Write-BootstrapLog([string]$Level, [string]$Message) {
    $line = '[{0}] [{1}] {2}' -f ([DateTimeOffset]::Now.ToString('yyyy-MM-dd HH:mm:ss')), $Level, $Message
    Write-Host $line
    Add-Content -LiteralPath $LogPath -Value $line -Encoding UTF8
}

function Get-DriveRootFromRepo {
    return [System.IO.Path]::GetPathRoot($RepoRoot)
}

function Test-PortableDriveRootPolicy {
    $portablePolicyPath = Join-Path $RepoRoot 'config\cortex\portable_drive_root_vault.v1.json'
    if (-not (Test-Path -LiteralPath $portablePolicyPath -PathType Leaf)) { return $false }
    try {
        $portablePolicy = Get-Content -LiteralPath $portablePolicyPath -Raw -Encoding UTF8 | ConvertFrom-Json
        return ([bool]$portablePolicy.enabled -and [string]$portablePolicy.mode -eq 'repository_drive_root')
    } catch {
        Write-BootstrapLog 'WARN' ('Unable to read portable drive-root policy: ' + $_.Exception.Message)
        return $false
    }
}

function Resolve-CortexVaultRoot {
    param([string]$ExplicitRoot)
    if (-not [string]::IsNullOrWhiteSpace($ExplicitRoot)) {
        return [System.IO.Path]::GetFullPath($ExplicitRoot)
    }
    if (-not [string]::IsNullOrWhiteSpace($env:CORTEX_VAULT_ROOT)) {
        return [System.IO.Path]::GetFullPath($env:CORTEX_VAULT_ROOT)
    }
    if (-not [string]::IsNullOrWhiteSpace($env:PCC_VAULT_ROOT)) {
        return [System.IO.Path]::GetFullPath($env:PCC_VAULT_ROOT)
    }

    $repoDrive = Get-DriveRootFromRepo
    if ((Test-PortableDriveRootPolicy) -and -not [string]::IsNullOrWhiteSpace($repoDrive)) {
        return [System.IO.Path]::GetFullPath($repoDrive)
    }

    $repoDrive = Get-DriveRootFromRepo
    if (-not [string]::IsNullOrWhiteSpace($repoDrive)) {
        $driveMarker = Join-Path $repoDrive '.cortex-vault-root.json'
        if (Test-Path -LiteralPath $driveMarker -PathType Leaf) {
            return [System.IO.Path]::GetFullPath($repoDrive)
        }
    }

    $policyPath = Join-Path $RepoRoot 'config\cortex\storage_policy.v1.json'
    if (Test-Path -LiteralPath $policyPath -PathType Leaf) {
        try {
            $policy = Get-Content -LiteralPath $policyPath -Raw -Encoding UTF8 | ConvertFrom-Json
            $configured = [string]$policy.default_library_root_windows
            if (-not [string]::IsNullOrWhiteSpace($configured)) {
                $drive = [System.IO.Path]::GetPathRoot($configured)
                if (-not [string]::IsNullOrWhiteSpace($drive) -and (Test-Path -LiteralPath $drive -PathType Container)) {
                    return [System.IO.Path]::GetFullPath($configured)
                }
            }
        } catch {
            Write-BootstrapLog 'WARN' ('Unable to read storage policy: ' + $_.Exception.Message)
        }
    }

    $local = $env:LOCALAPPDATA
    if ([string]::IsNullOrWhiteSpace($local)) {
        $local = Join-Path $env:USERPROFILE 'AppData\Local'
    }
    return [System.IO.Path]::GetFullPath((Join-Path $local 'Cortex\Vault'))
}

function Test-CortexPythonRuntime {
    param([string]$Exe, [switch]$RequireTk)
    if ([string]::IsNullOrWhiteSpace($Exe) -or -not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $false }
    try {
        if ($RequireTk) {
            & $Exe -c "import sys, tkinter; raise SystemExit(0 if sys.version_info >= (3,11) else 1)" *> $null
        } else {
            & $Exe -c "import sys; raise SystemExit(0 if sys.version_info >= (3,11) else 1)" *> $null
        }
        return ($LASTEXITCODE -eq 0)
    } catch { return $false }
}

function Get-PythonMigrationCandidates {
    param([string]$TargetHome)
    $candidates = New-Object System.Collections.Generic.List[string]

    foreach ($registryPath in @(
        'HKCU:\Software\Python\PythonCore\3.14\InstallPath',
        'HKLM:\Software\Python\PythonCore\3.14\InstallPath',
        'HKLM:\Software\WOW6432Node\Python\PythonCore\3.14\InstallPath'
    )) {
        try {
            if (Test-Path $registryPath) {
                $installPath = (Get-Item -Path $registryPath).GetValue('')
                if (-not [string]::IsNullOrWhiteSpace([string]$installPath)) {
                    $exe = Join-Path ([string]$installPath) 'python.exe'
                    if (Test-Path -LiteralPath $exe -PathType Leaf) { $candidates.Add($exe) }
                }
            }
        } catch { }
    }

    $legacyVaultPython = Join-Path $env:LOCALAPPDATA 'Cortex\Vault\shared\toolchains\python\3.14.7\python.exe'
    if (Test-Path -LiteralPath $legacyVaultPython -PathType Leaf) { $candidates.Add($legacyVaultPython) }

    $localPrograms = Join-Path $env:LOCALAPPDATA 'Programs\Python'
    if (Test-Path -LiteralPath $localPrograms -PathType Container) {
        try {
            Get-ChildItem -LiteralPath $localPrograms -Filter python.exe -File -Recurse -ErrorAction SilentlyContinue |
                ForEach-Object { $candidates.Add($_.FullName) }
        } catch { }
    }

    $systemPython = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($systemPython -and $systemPython.Source -notmatch '(?i)\\Microsoft\\WindowsApps\\python(?:3)?\.exe$') {
        $candidates.Add($systemPython.Source)
    }

    return $candidates |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Unique |
        Where-Object { (Split-Path -Parent $_) -ne $TargetHome }
}

function Copy-CortexPythonRuntime {
    param([string]$SourceExe, [string]$TargetHome)
    $sourceHome = Split-Path -Parent $SourceExe
    if ([string]::IsNullOrWhiteSpace($sourceHome) -or -not (Test-Path -LiteralPath $sourceHome -PathType Container)) { return $false }
    Write-BootstrapLog 'INFO' ("Migrating complete Python runtime from $sourceHome to $TargetHome")
    if (Test-Path -LiteralPath $TargetHome -PathType Container) { Remove-Item -LiteralPath $TargetHome -Recurse -Force }
    New-Item -ItemType Directory -Path $TargetHome -Force | Out-Null
    Copy-Item -Path (Join-Path $sourceHome '*') -Destination $TargetHome -Recurse -Force
    $targetExe = Join-Path $TargetHome 'python.exe'
    return (Test-CortexPythonRuntime -Exe $targetExe -RequireTk)
}

function Ensure-Directory([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        New-Item -ItemType Directory -Path $Path -Force | Out-Null
    }
}

function Download-File {
    param([string]$Uri, [string]$Destination, [switch]$NoCache)
    Ensure-Directory (Split-Path -Parent $Destination)
    Write-BootstrapLog 'INFO' ("Downloading $Uri")
    $headers = @{ 'User-Agent' = 'CortexBootstrap/1.1' }
    if ($NoCache) { $headers['Cache-Control'] = 'no-cache'; $headers['Pragma'] = 'no-cache' }
    Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $Destination -Headers $headers
}

function Get-RemoteSha256 {
    param([string]$ChecksumUri)
    $headers = @{ 'User-Agent' = 'CortexBootstrap/1.2'; 'Cache-Control' = 'no-cache'; 'Pragma' = 'no-cache' }
    $response = Invoke-WebRequest -UseBasicParsing -Uri $ChecksumUri -Headers $headers
    if ($response.Content -is [byte[]]) {
        $text = [System.Text.Encoding]::ASCII.GetString([byte[]]$response.Content)
    } else {
        $text = [string]$response.Content
    }
    $match = [regex]::Match($text, '(?i)(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])')
    if (-not $match.Success) {
        $preview = ($text -replace '[\r\n]+', ' ').Trim()
        if ($preview.Length -gt 160) { $preview = $preview.Substring(0, 160) + '...' }
        throw "No SHA-256 digest found at $ChecksumUri (response preview: $preview)"
    }
    return $match.Value.ToLowerInvariant()
}

function Ensure-VerifiedSha256Download {
    param(
        [string]$Uri,
        [string]$ChecksumUri,
        [string]$Destination,
        [int]$Attempts = 3
    )
    $lastReason = 'verification did not run'
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        try {
            $expected = Get-RemoteSha256 $ChecksumUri
            if (Test-Path -LiteralPath $Destination -PathType Leaf) {
                $existing = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
                if ($existing -eq $expected) {
                    Write-BootstrapLog 'INFO' ("Verified cached download: $Destination")
                    return $expected
                }
                Write-BootstrapLog 'WARN' ("Cached download SHA-256 mismatch (attempt $attempt/$Attempts); deleting stale file and downloading again. expected=$expected actual=$existing")
                Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
            }

            Download-File $Uri $Destination -NoCache
            $actual = (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -eq $expected) { return $expected }

            $lastReason = "SHA-256 mismatch expected=$expected actual=$actual"
            Write-BootstrapLog 'WARN' ("Verified download mismatch (attempt $attempt/$Attempts): $lastReason")
            Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
            if ($attempt -lt $Attempts) { Start-Sleep -Seconds ([Math]::Min(2 * $attempt, 5)) }
        } catch {
            $lastReason = $_.Exception.Message
            Write-BootstrapLog 'WARN' ("Verified download attempt $attempt/$Attempts failed: $lastReason")
            Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
            if ($attempt -lt $Attempts) { Start-Sleep -Seconds ([Math]::Min(2 * $attempt, 5)) }
        }
    }
    throw "Unable to download and verify $Uri after $Attempts attempts: $lastReason"
}

function Assert-AuthenticodeIfAvailable([string]$Path) {
    try {
        $sig = Get-AuthenticodeSignature -LiteralPath $Path
        if ($sig.Status -eq 'Valid') {
            return $true
        }
        Write-BootstrapLog 'WARN' ("Authenticode status for $Path is $($sig.Status).")
    } catch {
        Write-BootstrapLog 'WARN' ('Authenticode verification unavailable: ' + $_.Exception.Message)
    }
    return $false
}

function Get-ExeVersion {
    param([string]$Exe, [string[]]$Args)
    if ([string]::IsNullOrWhiteSpace($Exe) -or -not (Test-Path -LiteralPath $Exe -PathType Leaf)) { return $null }
    try {
        $output = & $Exe @Args 2>&1 | Select-Object -First 1
        return [string]$output
    } catch { return $null }
}

$Vault = Resolve-CortexVaultRoot $VaultRoot
$Shared = Join-Path $Vault 'shared'
$Toolchains = Join-Path $Shared 'toolchains'
$Downloads = Join-Path $Shared 'downloads\cortex-bootstrap'
$CargoHome = Join-Path $Shared 'dependencies\rust\cargo-home'
$RustupHome = Join-Path $Toolchains 'rust\rustup-home'
$PythonHome = Join-Path $Toolchains 'python\3.14.7'
$GitHome = Join-Path $Toolchains 'git\mingit'
$PipCache = Join-Path $Shared 'dependencies\python\pip-cache'
$NpmCache = Join-Path $Shared 'dependencies\node\npm-cache'
$SccacheDir = Join-Path $Shared 'dependencies\rust\sccache'
$SccacheHome = Join-Path $Toolchains 'sccache'
$GradleHome = Join-Path $Shared 'dependencies\java\gradle-home'
$NugetPackages = Join-Path $Shared 'dependencies\dotnet\nuget-packages'
$VcpkgDownloads = Join-Path $Shared 'dependencies\cpp\vcpkg-downloads'
$VcpkgBinaryCache = Join-Path $Shared 'dependencies\cpp\vcpkg-binary-cache'

foreach ($dir in @($Vault,$Shared,$Toolchains,$Downloads,$CargoHome,$RustupHome,$PipCache,$NpmCache,$SccacheDir,$SccacheHome,$GradleHome,$NugetPackages,$VcpkgDownloads,$VcpkgBinaryCache)) {
    Ensure-Directory $dir
}

$PortableAuthority = -not [string]::IsNullOrWhiteSpace($env:CORTEX_VAULT_ROOT)
if (Test-PortableDriveRootPolicy) { $PortableAuthority = $true }
$repoDriveMarker = Join-Path (Get-DriveRootFromRepo) '.cortex-vault-root.json'
if (Test-Path -LiteralPath $repoDriveMarker -PathType Leaf) { $PortableAuthority = $true }
$AllowNetwork = -not $NoNetwork
$ShouldHydrate = ($Mode -eq 'Startup' -or $Mode -eq 'Hydrate' -or $Mode -eq 'Full')

# Python: project-owned full install is preferred whenever an explicit/drive-root Vault is active.
# Cortex PCC GUI requires Tkinter, so the minimal embeddable Python ZIP is intentionally not used.
$PythonExe = Join-Path $PythonHome 'python.exe'
$PythonwExe = Join-Path $PythonHome 'pythonw.exe'
$PythonOk = Test-CortexPythonRuntime -Exe $PythonExe -RequireTk
if (-not $PythonOk -and -not $PortableAuthority) {
    $systemPython = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($systemPython -and $systemPython.Source -notmatch '(?i)\\Microsoft\\WindowsApps\\python(?:3)?\.exe$') {
        if (Test-CortexPythonRuntime -Exe $systemPython.Source -RequireTk) {
            $PythonExe = $systemPython.Source
            $candidateW = Join-Path (Split-Path -Parent $PythonExe) 'pythonw.exe'
            if (Test-Path -LiteralPath $candidateW) { $PythonwExe = $candidateW }
            $PythonOk = $true
        }
    }
}
if (-not $PythonOk -and $ShouldHydrate -and $AllowNetwork) {
    try {
        $installer = Join-Path $Downloads 'python-3.14.7-amd64.exe'
        if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
            Download-File 'https://www.python.org/ftp/python/3.14.7/python-3.14.7-amd64.exe' $installer
        }
        Assert-AuthenticodeIfAvailable $installer | Out-Null
        Write-BootstrapLog 'INFO' ("Hydrating full Python 3.14.7 runtime into $PythonHome")
        Ensure-Directory $PythonHome
        $args = @('/quiet','InstallAllUsers=0','PrependPath=0','Include_launcher=0','Include_pip=1','Include_test=0','Include_doc=0','Include_tcltk=1','Include_exe=1','Include_lib=1','Shortcuts=0',("TargetDir=$PythonHome"))
        $proc = Start-Process -FilePath $installer -ArgumentList $args -Wait -PassThru
        if ($proc.ExitCode -ne 0) { Write-BootstrapLog 'WARN' ("Python installer exited $($proc.ExitCode).") }
        $PythonOk = Test-CortexPythonRuntime -Exe $PythonExe -RequireTk

        # The traditional installer is single-instance aware. If this Python version was already
        # installed by an earlier Cortex bootstrap (for example the old AppData Vault), the installer
        # may exit successfully without creating a second TargetDir. In that case migrate the complete
        # registered/runtime tree to the active drive-root Vault and validate it in-place.
        if (-not $PythonOk -and $PortableAuthority) {
            foreach ($candidate in (Get-PythonMigrationCandidates -TargetHome $PythonHome)) {
                if (-not (Test-CortexPythonRuntime -Exe $candidate -RequireTk)) { continue }
                try {
                    if (Copy-CortexPythonRuntime -SourceExe $candidate -TargetHome $PythonHome) {
                        $PythonExe = Join-Path $PythonHome 'python.exe'
                        $PythonwExe = Join-Path $PythonHome 'pythonw.exe'
                        $PythonOk = $true
                        Write-BootstrapLog 'INFO' ("Portable Python migration validated: $PythonExe")
                        break
                    }
                } catch {
                    Write-BootstrapLog 'WARN' ("Python migration candidate failed: $candidate :: $($_.Exception.Message)")
                }
            }
        }

        # Final recursive rediscovery covers installer layout changes while still validating Tkinter.
        if (-not $PythonOk -and (Test-Path -LiteralPath $PythonHome -PathType Container)) {
            foreach ($candidate in (Get-ChildItem -LiteralPath $PythonHome -Filter python.exe -File -Recurse -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)) {
                if (Test-CortexPythonRuntime -Exe $candidate -RequireTk) {
                    $PythonExe = $candidate
                    $candidateW = Join-Path (Split-Path -Parent $PythonExe) 'pythonw.exe'
                    if (Test-Path -LiteralPath $candidateW -PathType Leaf) { $PythonwExe = $candidateW }
                    $PythonOk = $true
                    break
                }
            }
        }
        if (-not $PythonOk) {
            throw "Python hydration completed but no working Python 3.11+ runtime with Tkinter was discovered under $PythonHome or migration candidates"
        }
    } catch {
        Write-BootstrapLog 'WARN' ('Python hydration failed: ' + $_.Exception.Message)
        $PythonOk = $false
    }
}

# Git: use Vault MinGit under portable authority, otherwise accept a system Git.
$GitExe = Join-Path $GitHome 'cmd\git.exe'
$GitOk = Test-Path -LiteralPath $GitExe -PathType Leaf
if (-not $GitOk -and -not $PortableAuthority) {
    $systemGit = Get-Command git.exe -ErrorAction SilentlyContinue
    if ($systemGit) { $GitExe = $systemGit.Source; $GitOk = $true }
}
if (-not $GitOk -and $ShouldHydrate -and $AllowNetwork) {
    try {
        Write-BootstrapLog 'INFO' 'Resolving current stable Git for Windows MinGit release.'
        $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/git-for-windows/git/releases/latest' -Headers @{ 'User-Agent' = 'CortexBootstrap/1.1' }
        $asset = $release.assets | Where-Object { $_.name -match '^MinGit-.*-64-bit\.zip$' -and $_.name -notmatch 'busybox' } | Select-Object -First 1
        if (-not $asset) { throw 'Unable to locate a stable 64-bit MinGit release asset.' }
        $zip = Join-Path $Downloads $asset.name
        if (-not (Test-Path -LiteralPath $zip -PathType Leaf)) { Download-File $asset.browser_download_url $zip }
        if ($asset.digest -and ([string]$asset.digest).StartsWith('sha256:')) {
            $expected = ([string]$asset.digest).Substring(7).ToLowerInvariant()
            $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($expected -ne $actual) {
                Write-BootstrapLog 'WARN' 'Cached MinGit SHA-256 mismatch; deleting stale archive for the next bootstrap attempt.'
                Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue
                throw 'MinGit SHA-256 verification failed.'
            }
        }
        if (Test-Path -LiteralPath $GitHome) { Remove-Item -LiteralPath $GitHome -Recurse -Force }
        Ensure-Directory $GitHome
        Expand-Archive -LiteralPath $zip -DestinationPath $GitHome -Force
        $GitOk = Test-Path -LiteralPath $GitExe -PathType Leaf
    } catch {
        Write-BootstrapLog 'WARN' ('Git hydration failed; PCC can still launch if Python is ready: ' + $_.Exception.Message)
        $GitOk = $false
    }
}

# Rust: shared Cargo downloads plus Vault-owned rustup toolchain.
$RustupExe = Join-Path $CargoHome 'bin\rustup.exe'
$CargoExe = Join-Path $CargoHome 'bin\cargo.exe'
$RustcExe = Join-Path $CargoHome 'bin\rustc.exe'
$RustOk = (Test-Path -LiteralPath $CargoExe -PathType Leaf) -and (Test-Path -LiteralPath $RustcExe -PathType Leaf)
if (-not $RustOk -and -not $PortableAuthority) {
    $systemCargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
    $systemRustc = Get-Command rustc.exe -ErrorAction SilentlyContinue
    if ($systemCargo -and $systemRustc) {
        $CargoExe = $systemCargo.Source
        $RustcExe = $systemRustc.Source
        $RustOk = $true
    }
}
if (-not $RustOk -and $ShouldHydrate -and $AllowNetwork) {
    try {
        $rustupInit = Join-Path $Downloads 'rustup-init-x86_64-pc-windows-msvc.exe'
        $rustupUrl = 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe'
        $rustupShaUrl = $rustupUrl + '.sha256'
        $verifiedHash = Ensure-VerifiedSha256Download -Uri $rustupUrl -ChecksumUri $rustupShaUrl -Destination $rustupInit -Attempts 3
        Write-BootstrapLog 'INFO' ("rustup-init SHA-256 verified: $verifiedHash")
        $env:RUSTUP_HOME = $RustupHome
        $env:CARGO_HOME = $CargoHome
        Write-BootstrapLog 'INFO' ("Hydrating Rust stable into $RustupHome")
        & $rustupInit -y --no-modify-path --profile minimal --default-toolchain stable
        if ($LASTEXITCODE -ne 0) { throw "rustup-init failed with exit code $LASTEXITCODE" }
        $RustupExe = Join-Path $CargoHome 'bin\rustup.exe'
        & $RustupExe component add rustfmt clippy
        if ($LASTEXITCODE -ne 0) { throw "rustup component hydration failed with exit code $LASTEXITCODE" }
        $CargoExe = Join-Path $CargoHome 'bin\cargo.exe'
        $RustcExe = Join-Path $CargoHome 'bin\rustc.exe'
        $RustOk = (Test-Path -LiteralPath $CargoExe -PathType Leaf) -and (Test-Path -LiteralPath $RustcExe -PathType Leaf)
    } catch {
        Write-BootstrapLog 'WARN' ('Rust hydration failed; preserving Python/Git environment so PCC can still launch: ' + $_.Exception.Message)
        $RustOk = $false
    }
}

# Final Rust component checks are non-mutating unless hydration is enabled.
$RustfmtOk = $false
$ClippyOk = $false
if ($RustOk) {
    $env:RUSTUP_HOME = $RustupHome
    $env:CARGO_HOME = $CargoHome
    $binDir = Split-Path -Parent $CargoExe
    if (Test-Path -LiteralPath (Join-Path $binDir 'rustfmt.exe')) { $RustfmtOk = $true }
    if (Test-Path -LiteralPath (Join-Path $binDir 'cargo-clippy.exe')) { $ClippyOk = $true }
}

# MSVC Build Tools are needed for Windows Rust linking. They are machine-scoped Windows
# dependencies, not portable Vault payloads. Cortex keeps the bootstrapper/download cache in the
# Vault, but Visual Studio Setup is allowed to install its required product/shared components on
# supported local Windows locations. Cargo then inherits the activated developer environment.
function Test-IsAdministrator {
    try {
        $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
        $principal = New-Object Security.Principal.WindowsPrincipal($identity)
        return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    } catch { return $false }
}

function Resolve-MsvcInstallPath {
    $vsWhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vsWhere -PathType Leaf) {
        try {
            $found = (& $vsWhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
            if (-not [string]::IsNullOrWhiteSpace([string]$found)) { return [string]$found }
        } catch { }
    }
    foreach ($candidate in @(
        (Join-Path $env:ProgramFiles 'Microsoft Visual Studio\2022\BuildTools'),
        (Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\2022\BuildTools')
    )) {
        if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path -LiteralPath $candidate -PathType Container)) {
            return $candidate
        }
    }
    return $null
}

function Get-VsDevCmdPath {
    param([string]$InstallPath)
    if ([string]::IsNullOrWhiteSpace($InstallPath)) { return $null }
    $candidate = Join-Path $InstallPath 'Common7\Tools\VsDevCmd.bat'
    if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    return $null
}

function Test-MsvcDeveloperEnvironment {
    param([string]$VsDevCmd)
    if ([string]::IsNullOrWhiteSpace($VsDevCmd) -or -not (Test-Path -LiteralPath $VsDevCmd -PathType Leaf)) { return $false }
    try {
        $cmdLine = 'call "' + $VsDevCmd + '" -no_logo -arch=amd64 -host_arch=amd64 >nul && where link.exe >nul 2>nul && where cl.exe >nul 2>nul'
        & $env:ComSpec /d /s /c $cmdLine *> $null
        return ($LASTEXITCODE -eq 0)
    } catch { return $false }
}

function Get-VsInstallerExitMeaning {
    param([int]$Code)
    switch ($Code) {
        0 { return 'success' }
        740 { return 'elevation required' }
        1001 { return 'Visual Studio Installer already running' }
        1003 { return 'Visual Studio is in use' }
        1602 { return 'operation canceled' }
        1618 { return 'another installation is running' }
        1641 { return 'success; reboot initiated' }
        3010 { return 'success; reboot required' }
        5003 { return 'bootstrapper could not download installer' }
        5004 { return 'operation canceled' }
        5005 { return 'bootstrapper command-line parse error' }
        5007 { return 'machine does not meet requirements' }
        8004 { return 'target directory failure' }
        8005 { return 'source payload verification failure' }
        8006 { return 'Visual Studio processes are running' }
        8010 { return 'operating system not supported' }
        default { return 'installer failure/unknown result' }
    }
}

function Copy-VisualStudioSetupEvidence {
    param([datetime]$Since)
    $dest = Join-Path $Artifacts 'visual-studio-setup'
    Ensure-Directory $dest
    try {
        Get-ChildItem -LiteralPath $env:TEMP -File -ErrorAction SilentlyContinue |
            Where-Object { $_.LastWriteTimeUtc -ge $Since.ToUniversalTime() -and $_.Name -match '^(dd_(bootstrapper|setup|vsixinstaller)|vslogs).*\.log$' } |
            Sort-Object LastWriteTimeUtc |
            ForEach-Object {
                Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $dest $_.Name) -Force -ErrorAction SilentlyContinue
            }
    } catch { }
    return $dest
}

$LegacyPortableMsvcPath = Join-Path $Toolchains 'vs2022-buildtools'
if (Test-Path -LiteralPath $LegacyPortableMsvcPath -PathType Container) {
    Write-BootstrapLog 'WARN' ("Legacy portable MSVC directory exists at $LegacyPortableMsvcPath; it is not used as Build Tools authority and will not be deleted automatically.")
}

$MsvcPath = Resolve-MsvcInstallPath
$VsDevCmd = Get-VsDevCmdPath -InstallPath $MsvcPath
$MsvcOk = Test-MsvcDeveloperEnvironment -VsDevCmd $VsDevCmd
$MsvcInstallExitCode = $null
$MsvcRebootRequired = $false

if ($IncludeBuildTools -and -not $MsvcOk -and $ShouldHydrate -and $AllowNetwork) {
    # Visual Studio Setup requires administrative elevation. Relaunch this bootstrap elevated before
    # invoking the installer so quiet/passive setup cannot fail or return early due to UAC policy.
    if (-not (Test-IsAdministrator)) {
        Write-BootstrapLog 'INFO' 'MSVC hydration requires elevation; requesting a Windows administrator prompt.'
        $elevatedArgs = @('-NoProfile','-ExecutionPolicy','Bypass','-File',('"{0}"' -f $PSCommandPath),'-Mode',$Mode,'-IncludeBuildTools')
        if (-not [string]::IsNullOrWhiteSpace($VaultRoot)) {
            $elevatedArgs += @('-VaultRoot',('"{0}"' -f $VaultRoot))
        }
        if ($NoNetwork) { $elevatedArgs += '-NoNetwork' }
        try {
            $elevated = Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $elevatedArgs -Wait -PassThru
            exit $elevated.ExitCode
        } catch {
            Write-BootstrapLog 'WARN' ('Unable to elevate MSVC hydration: ' + $_.Exception.Message)
        }
    }

    $vsInstaller = Join-Path $Downloads 'vs_BuildTools.exe'
    if (-not (Test-Path -LiteralPath $vsInstaller -PathType Leaf)) { Download-File 'https://aka.ms/vs/17/release/vs_BuildTools.exe' $vsInstaller }
    Assert-AuthenticodeIfAvailable $vsInstaller | Out-Null

    # Keep the downloaded bootstrapper in the E:/Vault cache, but do not force Visual Studio itself
    # onto a removable drive. Visual Studio Setup has system/shared components that are machine-local.
    $installStart = [DateTime]::UtcNow
    Write-BootstrapLog 'INFO' ("Hydrating machine-scoped Visual Studio 2022 C++ Build Tools; Cortex download cache remains at $Downloads")
    Write-BootstrapLog 'INFO' 'Visual Studio Setup is running with passive progress and may take several minutes.'
    $vsArgs = @(
        '--passive','--wait','--norestart','--nocache',
        '--add','Microsoft.VisualStudio.Workload.VCTools',
        '--add','Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
        '--includeRecommended'
    )
    $proc = Start-Process -FilePath $vsInstaller -ArgumentList $vsArgs -Wait -PassThru
    $MsvcInstallExitCode = [int]$proc.ExitCode
    $exitMeaning = Get-VsInstallerExitMeaning -Code $MsvcInstallExitCode
    Write-BootstrapLog 'INFO' ("Visual Studio Build Tools installer exit code: $MsvcInstallExitCode ($exitMeaning)")
    $setupEvidence = Copy-VisualStudioSetupEvidence -Since $installStart
    Write-BootstrapLog 'INFO' ("Visual Studio setup evidence: $setupEvidence")

    if ($MsvcInstallExitCode -eq 3010 -or $MsvcInstallExitCode -eq 1641) {
        $MsvcRebootRequired = $true
        Write-BootstrapLog 'WARN' 'Visual Studio Build Tools reported that Windows must be restarted before the installation is fully usable.'
    } elseif ($MsvcInstallExitCode -ne 0) {
        Write-BootstrapLog 'WARN' ("Visual Studio Build Tools installation did not complete successfully: $exitMeaning")
    }

    $MsvcPath = Resolve-MsvcInstallPath
    $VsDevCmd = Get-VsDevCmdPath -InstallPath $MsvcPath
    $MsvcOk = Test-MsvcDeveloperEnvironment -VsDevCmd $VsDevCmd
    if ($MsvcOk) {
        Write-BootstrapLog 'INFO' ("MSVC amd64 developer environment verified at $MsvcPath")
    } elseif ($MsvcRebootRequired) {
        Write-BootstrapLog 'WARN' 'MSVC is not active yet because Visual Studio Setup requested a reboot.'
    } else {
        Write-BootstrapLog 'WARN' 'Visual Studio Build Tools are still unavailable after setup; inspect the captured Visual Studio setup logs.'
    }
}

# Shared Rust compiled-artifact cache. This is an accelerator, not a launch blocker.
# sccache keys compiler/version/features/input content, so exact-compatible dependencies can
# be reused across projects while different versions/features naturally produce different keys.
$SccacheExe = Join-Path $SccacheHome 'bin\sccache.exe'
$SccacheOk = Test-Path -LiteralPath $SccacheExe -PathType Leaf
if (-not $SccacheOk) {
    $systemSccache = Get-Command sccache.exe -ErrorAction SilentlyContinue
    if ($systemSccache) { $SccacheExe = $systemSccache.Source; $SccacheOk = $true }
}
if (-not $SccacheOk -and $RustOk -and $MsvcOk -and $ShouldHydrate -and $AllowNetwork) {
    try {
        Write-BootstrapLog 'INFO' ("Hydrating shared Rust compiled-cache accelerator into $SccacheHome")
        Ensure-Directory $SccacheHome
        $installLine = 'call "' + $VsDevCmd + '" -no_logo -arch=amd64 -host_arch=amd64 >nul && set "CARGO_HOME=' + $CargoHome + '" && set "RUSTUP_HOME=' + $RustupHome + '" && "' + $CargoExe + '" install sccache --locked --root "' + $SccacheHome + '"'
        & $env:ComSpec /d /s /c $installLine
        if ($LASTEXITCODE -ne 0) { throw "cargo install sccache failed with exit code $LASTEXITCODE" }
        $SccacheExe = Join-Path $SccacheHome 'bin\sccache.exe'
        $SccacheOk = Test-Path -LiteralPath $SccacheExe -PathType Leaf
    } catch {
        Write-BootstrapLog 'WARN' ('sccache hydration failed; dependency downloads remain shared but compiled reuse is unavailable: ' + $_.Exception.Message)
        $SccacheOk = $false
    }
}

# Persist environment handoff for the CMD launcher and child processes.
$pathParts = New-Object System.Collections.Generic.List[string]
if ($PythonOk) {
    $pathParts.Add((Split-Path -Parent $PythonExe))
    $scriptsDir = Join-Path (Split-Path -Parent $PythonExe) 'Scripts'
    if (Test-Path -LiteralPath $scriptsDir -PathType Container) { $pathParts.Add($scriptsDir) }
}
if ($GitOk) { $pathParts.Add((Split-Path -Parent $GitExe)) }
if ($RustOk -and (Test-Path -LiteralPath $CargoHome -PathType Container)) { $pathParts.Add((Join-Path $CargoHome 'bin')) }
if ($SccacheOk) { $pathParts.Add((Split-Path -Parent $SccacheExe)) }

$envLines = New-Object System.Collections.Generic.List[string]
$envLines.Add('@echo off')
$PortableHome = Join-Path $Vault '.cortex\home'
$PortableProjects = Join-Path $Vault 'Source'
$PortableModels = Join-Path $Vault 'Models'
foreach ($portableDir in @($PortableHome, $PortableProjects, $PortableModels)) {
    New-Item -ItemType Directory -Path $portableDir -Force | Out-Null
}

# One-time migration of legacy per-machine Cortex state into the portable drive.
$LegacyHome = $null
if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    $LegacyHome = Join-Path $env:LOCALAPPDATA 'Cortex'
}
$PortableHomeHasState = Test-Path -LiteralPath (Join-Path $PortableHome 'registry') -PathType Container
if (-not $PortableHomeHasState -and $LegacyHome -and (Test-Path -LiteralPath $LegacyHome -PathType Container)) {
    try {
        Write-BootstrapLog 'INFO' ('Migrating legacy Cortex state into portable home: ' + $PortableHome)
        Copy-Item -Path (Join-Path $LegacyHome '*') -Destination $PortableHome -Recurse -Force -ErrorAction Stop
    } catch {
        Write-BootstrapLog 'WARN' ('Portable Cortex-home migration skipped/partial: ' + $_.Exception.Message)
    }
}

$envLines.Add(('set "CORTEX_VAULT_ROOT={0}"' -f $Vault))
$envLines.Add(('set "PCC_VAULT_ROOT={0}"' -f $Vault))
$envLines.Add(('set "CORTEX_HOME={0}"' -f $PortableHome))
$envLines.Add(('set "CORTEX_RUNTIME_ROOT={0}"' -f $RepoRoot))
$envLines.Add(('set "CORTEX_PROJECTS_ROOT={0}"' -f $PortableProjects))
$envLines.Add(('set "CORTEX_MODELS_ROOT={0}"' -f $PortableModels))
$envLines.Add('set "CORTEX_STATE_MODE=portable"')
$envLines.Add(('set "CARGO_HOME={0}"' -f $CargoHome))
$envLines.Add(('set "RUSTUP_HOME={0}"' -f $RustupHome))
$envLines.Add(('set "PIP_CACHE_DIR={0}"' -f $PipCache))
$envLines.Add(('set "npm_config_cache={0}"' -f $NpmCache))
$envLines.Add(('set "SCCACHE_DIR={0}"' -f $SccacheDir))
$envLines.Add('set "CORTEX_DEPENDENCY_REUSE_POLICY=share-compatible-isolate-conflicts"')
if ($SccacheOk) { $envLines.Add(('set "RUSTC_WRAPPER={0}"' -f $SccacheExe)); $envLines.Add(('set "CORTEX_SCCACHE_EXE={0}"' -f $SccacheExe)) }
$envLines.Add(('set "GRADLE_USER_HOME={0}"' -f $GradleHome))
$envLines.Add(('set "NUGET_PACKAGES={0}"' -f $NugetPackages))
$envLines.Add(('set "VCPKG_DOWNLOADS={0}"' -f $VcpkgDownloads))
$envLines.Add(('set "VCPKG_DEFAULT_BINARY_CACHE={0}"' -f $VcpkgBinaryCache))
if ($PythonOk) { $envLines.Add(('set "CORTEX_PYTHON_EXE={0}"' -f $PythonExe)); if (Test-Path -LiteralPath $PythonwExe) { $envLines.Add(('set "CORTEX_PYTHONW_EXE={0}"' -f $PythonwExe)) } }
if ($GitOk) { $envLines.Add(('set "CORTEX_GIT_EXE={0}"' -f $GitExe)) }
if ($RustOk) { $envLines.Add(('set "CORTEX_CARGO_EXE={0}"' -f $CargoExe)); $envLines.Add(('set "CORTEX_RUSTC_EXE={0}"' -f $RustcExe)) }
if ($MsvcOk -and -not [string]::IsNullOrWhiteSpace($VsDevCmd)) {
    $envLines.Add(('set "CORTEX_VSDEVCMD={0}"' -f $VsDevCmd))
    $envLines.Add(('call "{0}" -no_logo -arch=amd64 -host_arch=amd64 ^>nul' -f $VsDevCmd))
}
if ($pathParts.Count -gt 0) { $envLines.Add(('set "PATH={0};%PATH%"' -f ([string]::Join(';', $pathParts)))) }
[System.IO.File]::WriteAllLines($EnvCmdPath, $envLines, (New-Object System.Text.UTF8Encoding($false)))

$health = [ordered]@{
    schema = 'cortex.environment_health.v1'
    generatedAt = [DateTimeOffset]::UtcNow.ToString('o')
    mode = $Mode
    repository = $RepoRoot
    vaultRoot = $Vault
    cortexHome = $PortableHome
    projectsRoot = $PortableProjects
    modelsRoot = $PortableModels
    portableAuthority = $PortableAuthority
    portableDriveRootPolicy = (Test-PortableDriveRootPolicy)
    environmentHandoff = $EnvCmdPath
    launchReady = $PythonOk
    buildReady = ($PythonOk -and $GitOk -and $RustOk -and $RustfmtOk -and $ClippyOk -and $MsvcOk)
    tools = [ordered]@{
        python = [ordered]@{ requiredFor = 'launch'; ok = $PythonOk; path = $PythonExe; version = (Get-ExeVersion $PythonExe @('--version')) }
        git = [ordered]@{ requiredFor = 'project_control'; ok = $GitOk; path = $GitExe; version = (Get-ExeVersion $GitExe @('--version')) }
        cargo = [ordered]@{ requiredFor = 'build'; ok = $RustOk; path = $CargoExe; version = (Get-ExeVersion $CargoExe @('--version')) }
        rustc = [ordered]@{ requiredFor = 'build'; ok = $RustOk; path = $RustcExe; version = (Get-ExeVersion $RustcExe @('--version')) }
        rustfmt = [ordered]@{ requiredFor = 'certify'; ok = $RustfmtOk }
        clippy = [ordered]@{ requiredFor = 'certify'; ok = $ClippyOk }
        sccache = [ordered]@{ requiredFor = 'optional_cross_project_compiled_reuse'; ok = $SccacheOk; path = $SccacheExe; cache = $SccacheDir; policy = 'exact-compatible compiler inputs only' }
        msvcBuildTools = [ordered]@{ requiredFor = 'windows_link'; scope = 'machine'; ok = $MsvcOk; path = $MsvcPath; vsDevCmd = $VsDevCmd; activated = $MsvcOk; installExitCode = $MsvcInstallExitCode; rebootRequired = $MsvcRebootRequired; hydrationCommand = 'HYDRATE_CORTEX_BUILD_TOOLS.cmd'; setupEvidence = (Join-Path $Artifacts 'visual-studio-setup'); legacyPortablePath = $LegacyPortableMsvcPath }
    }
    shared = [ordered]@{
        cargoHome = $CargoHome
        rustupHome = $RustupHome
        pipCache = $PipCache
        npmCache = $NpmCache
        sccache = $SccacheDir
        dependencyReusePolicy = 'share-compatible-isolate-conflicts'
        downloads = $Downloads
    }
}
$health | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $HealthPath -Encoding UTF8

Write-BootstrapLog 'INFO' ("Vault root: $Vault")
Write-BootstrapLog 'INFO' ("Python: $PythonOk | Git: $GitOk | Rust: $RustOk | rustfmt: $RustfmtOk | clippy: $ClippyOk | MSVC: $MsvcOk | sccache: $SccacheOk")
Write-BootstrapLog 'INFO' ("Environment report: $HealthPath")

if (-not $PythonOk) {
    Write-BootstrapLog 'FAIL' 'Python 3.11+ is unavailable and could not be hydrated. PCC cannot launch.'
    exit 11
}
if (-not $GitOk -or -not $RustOk -or -not $RustfmtOk -or -not $ClippyOk) {
    Write-BootstrapLog 'WARN' 'PCC launch is available, but one or more build/certification dependencies remain unavailable.'
    exit 0
}
if (-not $MsvcOk) {
    Write-BootstrapLog 'WARN' 'MSVC C++ Build Tools are not detected. PCC can launch, but Windows Rust linking may fail. Run HYDRATE_CORTEX_BUILD_TOOLS.cmd once if needed.'
}
exit 0
