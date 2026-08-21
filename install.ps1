# Installs the latest (or $env:EUMEAUS_VERSION-pinned) eumeaus release
# from GitHub Releases. See CLI.md for what gets installed and where.
#
#   irm https://raw.githubusercontent.com/RedRockerSE/eumeaus/main/install.ps1 | iex
#
# Env vars:
#   EUMEAUS_VERSION      pin to a specific release tag (e.g. v0.1.0) instead
#                        of resolving "latest"
#   EUMEAUS_INSTALL_DIR  where to put eumeaus.exe (default: %LOCALAPPDATA%\eumeaus)

$ErrorActionPreference = "Stop"

$Repo = "RedRockerSE/eumeaus"
$InstallDir = if ($env:EUMEAUS_INSTALL_DIR) { $env:EUMEAUS_INSTALL_DIR } else { "$env:LOCALAPPDATA\eumeaus" }

if (-not [Environment]::Is64BitOperatingSystem) {
    Write-Error ("No prebuilt eumeaus binary for 32-bit Windows yet. Build from source instead " +
        "- see CLI.md's Building and running section: " +
        "https://github.com/$Repo/blob/main/CLI.md#building-and-running")
    exit 1
}
$Target = "x86_64-pc-windows-msvc"

$Version = $env:EUMEAUS_VERSION
if (-not $Version) {
    Write-Host "Resolving the latest eumeaus release..."
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Release.tag_name
}
if (-not $Version) {
    Write-Error ("Could not determine the latest eumeaus release (rate-limited by GitHub's API? " +
        "Set the EUMEAUS_VERSION environment variable to a release tag, e.g. v0.1.0, to skip this lookup.)")
    exit 1
}

$Stage = "eumeaus-$Version-$Target"
# ${Stage} (not $Stage.zip, which PowerShell parses as member access on
# $Stage rather than concatenation) disambiguates the variable name.
$Archive = "${Stage}.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"

$TmpDir = Join-Path $env:TEMP "eumeaus-install-$(Get-Random)"
New-Item -ItemType Directory -Path $TmpDir | Out-Null

try {
    Write-Host "Downloading eumeaus $Version for $Target..."
    $ArchivePath = Join-Path $TmpDir $Archive
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ArchivePath
    $ChecksumsPath = Join-Path $TmpDir "checksums.txt"
    Invoke-WebRequest -Uri "$BaseUrl/checksums.txt" -OutFile $ChecksumsPath

    $ExpectedLine = Select-String -Path $ChecksumsPath -Pattern ([regex]::Escape($Archive)) | Select-Object -First 1
    if (-not $ExpectedLine) {
        Write-Error "No checksum entry for $Archive in checksums.txt - refusing to install an unverifiable download"
        exit 1
    }
    $Expected = ($ExpectedLine.Line -split '\s+')[0]
    $Actual = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLower()
    if ($Expected -ne $Actual) {
        Write-Error "Checksum mismatch for $Archive`n  expected: $Expected`n  actual:   $Actual"
        exit 1
    }
    Write-Host "Checksum verified."

    Expand-Archive -Path $ArchivePath -DestinationPath $TmpDir -Force
    $ExtractedDir = Join-Path $TmpDir $Stage

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "eumeaus.exe") (Join-Path $InstallDir "eumeaus.exe") -Force

    $PluginDir = Join-Path $InstallDir "eumeaus-plugins\username-search"
    New-Item -ItemType Directory -Path $PluginDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "plugins\username-search\*") $PluginDir -Force -Recurse

    $EmailPluginDir = Join-Path $InstallDir "eumeaus-plugins\email-lookup"
    New-Item -ItemType Directory -Path $EmailPluginDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "plugins\email-lookup\*") $EmailPluginDir -Force -Recurse

    $IpPluginDir = Join-Path $InstallDir "eumeaus-plugins\ip-lookup"
    New-Item -ItemType Directory -Path $IpPluginDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "plugins\ip-lookup\*") $IpPluginDir -Force -Recurse

    $DomainPluginDir = Join-Path $InstallDir "eumeaus-plugins\domain-lookup"
    New-Item -ItemType Directory -Path $DomainPluginDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "plugins\domain-lookup\*") $DomainPluginDir -Force -Recurse

    $CryptoWalletPluginDir = Join-Path $InstallDir "eumeaus-plugins\crypto-wallet"
    New-Item -ItemType Directory -Path $CryptoWalletPluginDir -Force | Out-Null
    Copy-Item (Join-Path $ExtractedDir "plugins\crypto-wallet\*") $CryptoWalletPluginDir -Force -Recurse

    Write-Host ""
    Write-Host "Installed eumeaus $Version to $InstallDir\eumeaus.exe"
    Write-Host "Installed the bundled username-search plugin to $PluginDir"
    Write-Host "Installed the bundled email-lookup plugin to $EmailPluginDir"
    Write-Host "Installed the bundled ip-lookup plugin to $IpPluginDir"
    Write-Host "Installed the bundled domain-lookup plugin to $DomainPluginDir"
    Write-Host "Installed the bundled crypto-wallet plugin to $CryptoWalletPluginDir"

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($UserPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
        Write-Host ""
        Write-Host "Added $InstallDir to your user PATH. Restart your terminal for it to take effect."
    }

    Write-Host ""
    Write-Host "Try it: eumeaus --help"
    Write-Host ("To run a scan with the bundled plugin: eumeaus scan run --plugins-dir " +
        "$InstallDir\eumeaus-plugins ...")
} finally {
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
}
