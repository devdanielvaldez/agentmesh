<#
.SYNOPSIS
  Installs the AgentMesh binary on Windows from GitHub Releases.
.DESCRIPTION
  Downloads the prebuilt x86_64 Windows zip, verifies its SHA-256 checksum,
  extracts it, and adds the install directory to the user PATH.
.EXAMPLE
  irm https://raw.githubusercontent.com/devdanielvaldez/agentmesh/main/install.ps1 | iex
.EXAMPLE
  & .\install.ps1 -Version v0.1.0 -InstallDir C:\tools\agentmesh
#>
[CmdletBinding()]
param(
  [string]$Version = "latest",
  [string]$InstallDir = (Join-Path $env:USERPROFILE ".agentmesh\bin")
)

$ErrorActionPreference = "Stop"
$Repo = "devdanielvaldez/agentmesh"
$Bin = "agentmesh.exe"

if ([Environment]::Is64BitOperatingSystem -eq $false -or
    ($env:PROCESSOR_ARCHITECTURE -ne "AMD64" -and $env:PROCESSOR_ARCHITEW6432 -ne "AMD64")) {
  throw "Only 64-bit x86 Windows has prebuilt binaries. Install Rust and run: cargo install --git https://github.com/$Repo --locked"
}
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if ($Version -eq "latest") {
  $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
  $Tag = $release.tag_name
  if (-not $Tag) { throw "Could not resolve latest release" }
} else {
  $Tag = $Version
}

$Triple = "x86_64-pc-windows-msvc"
$Asset = "agentmesh-$Tag-$Triple.zip"
$Base = "https://github.com/$Repo/releases/download/$Tag"
$Work = Join-Path ([IO.Path]::GetTempPath()) ("agentmesh-install-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $Work | Out-Null
try {
  Write-Host "Downloading $Asset ..."
  Invoke-WebRequest "$Base/$Asset" -OutFile (Join-Path $Work $Asset)
  Invoke-WebRequest "$Base/$Asset.sha256" -OutFile (Join-Path $Work "$Asset.sha256")
  $expected = ((Get-Content (Join-Path $Work "$Asset.sha256") -Raw) | Select-Object -First 1).Split()[0]
  $actual = (Get-FileHash (Join-Path $Work $Asset) -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $expected.ToLower()) { throw "Checksum mismatch for $Asset" }
  Write-Host "Checksum OK."

  New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
  Expand-Archive -Path (Join-Path $Work $Asset) -DestinationPath $InstallDir -Force
  & (Join-Path $InstallDir $Bin) --version
  Write-Host "Installed to $(Join-Path $InstallDir $Bin)"

  $path = [Environment]::GetEnvironmentVariable("PATH", "User")
  if (($path -split ";") -notcontains $InstallDir) {
    [Environment]::SetEnvironmentVariable("PATH", "$path;$InstallDir", "User")
    Write-Host "Added $InstallDir to your user PATH. Restart the terminal to use it."
  }
} finally {
  Remove-Item -Recurse -Force $Work -ErrorAction SilentlyContinue
}
