<#
.SYNOPSIS
  Crossthreads installer for Windows (no Rust/Node toolchain needed).

.EXAMPLE
  irm https://raw.githubusercontent.com/Cam-Smith-One/Crossthreads/main/scripts/install.ps1 | iex

  Installs to %USERPROFILE%\.crossthreads and adds it to your user PATH. Then:
    crossthreads-up            # start the app + web UI
    crossthreads search "..."  # or search from the terminal
#>

$ErrorActionPreference = "Stop"
$Repo   = if ($env:CROSSTHREADS_REPO) { $env:CROSSTHREADS_REPO } else { "Cam-Smith-One/Crossthreads" }
$Prefix = if ($env:CROSSTHREADS_PREFIX) { $env:CROSSTHREADS_PREFIX } else { Join-Path $env:USERPROFILE ".crossthreads" }
$Target = "x86_64-pc-windows-msvc"

# Resolve the release tag (a specific one, or the latest).
$Tag = $env:CROSSTHREADS_VERSION
if (-not $Tag) {
  $latest = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
  $Tag = $latest.tag_name
}
if (-not $Tag) { throw "No published release found for $Repo. Build from source instead." }

$Asset = "crossthreads-$Tag-$Target.zip"
$Url   = "https://github.com/$Repo/releases/download/$Tag/$Asset"
Write-Host "Installing Crossthreads $Tag for $Target"

$tmp = Join-Path $env:TEMP ("ct-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$zip = Join-Path $tmp "ct.zip"
Invoke-WebRequest -Uri $Url -OutFile $zip

if (Test-Path $Prefix) { Remove-Item -Recurse -Force $Prefix }
Expand-Archive -Path $zip -DestinationPath $tmp -Force
# The archive has a single top-level folder; move its contents into $Prefix.
$top = Get-ChildItem $tmp -Directory | Select-Object -First 1
New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
Copy-Item -Recurse -Force (Join-Path $top.FullName "*") $Prefix
Remove-Item -Recurse -Force $tmp

# Add the bin dir to the user PATH (idempotent).
$bin = Join-Path $Prefix "bin"
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$bin*") {
  [Environment]::SetEnvironmentVariable("Path", "$userPath;$bin", "User")
  Write-Host "Added $bin to your user PATH (restart your terminal to pick it up)."
}

Write-Host ""
Write-Host "Installed to $Prefix"
Write-Host "Next:  crossthreads-up        # open the app"
Write-Host "       crossthreads search `"oauth refresh`"   # or search in the terminal"
