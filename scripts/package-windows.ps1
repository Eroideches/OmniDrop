<#
.SYNOPSIS
    Packages `flutter build windows --release` as dist\omnidrop-windows-x64.zip (portable) and
    dist\omnidrop-windows-x64-setup.exe (Inno Setup installer that also configures the firewall).
#>
param([Parameter(Mandatory = $true)][string]$Version)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root 'build\windows\x64\runner\Release'
$dist = Join-Path $root 'dist'
$stage = Join-Path $root 'build\package\OmniDrop'

if (-not (Test-Path (Join-Path $release 'omnidrop.exe'))) { throw 'omnidrop.exe not found: run flutter build windows first' }
if (-not (Test-Path (Join-Path $release 'omnidrop_core.dll'))) { throw 'omnidrop_core.dll missing from the bundle' }

Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage, $dist | Out-Null
Copy-Item -Recurse -Path (Join-Path $release '*') -Destination $stage
Copy-Item (Join-Path $root 'packaging\windows\firewall-setup.ps1') $stage
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE.txt')

# App-local Visual C++ runtime (allowed by the VC++ redistributable licence) so the portable zip
# runs on machines without the redistributable installed.
foreach ($dll in 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') {
    $src = Join-Path $env:SystemRoot "System32\$dll"
    if (Test-Path $src) { Copy-Item $src $stage }
}

$zip = Join-Path $dist 'omnidrop-windows-x64.zip'
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $stage -DestinationPath $zip -CompressionLevel Optimal

$iscc = @(
    (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) {
    choco install innosetup -y --no-progress | Out-Null
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
& $iscc "/DAppVersion=$Version" "/DSourceDir=$stage" "/DOutputDir=$dist" (Join-Path $root 'packaging\windows\omnidrop.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }

Get-ChildItem $dist | Format-Table Name, Length
