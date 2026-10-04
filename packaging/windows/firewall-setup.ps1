<#
.SYNOPSIS
    Configures Windows Defender Firewall for OmniDrop (run once, needs administrator rights).

.DESCRIPTION
    Adds inbound rules for omnidrop.exe so that other devices can discover it (mDNS UDP 5353,
    UDP broadcast 44851) and connect to it (TCP 44850, or the ephemeral port used when 44850 is
    busy). Wi-Fi Direct links are classified by Windows as "Public" networks, so all profiles are
    allowed by default; use -PrivateOnly to restrict the rules to Domain and Private networks.
    Every transfer is still authenticated (Noise_XX + SAS) and must be accepted by the user.

.PARAMETER ExePath
    Full path of omnidrop.exe. Defaults to the executable next to this script.

.PARAMETER PrivateOnly
    Only allow Domain and Private network profiles.

.PARAMETER Remove
    Removes the OmniDrop rules.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File firewall-setup.ps1
#>
param(
    [string]$ExePath = (Join-Path $PSScriptRoot 'omnidrop.exe'),
    [switch]$PrivateOnly,
    [switch]$Remove
)
$ErrorActionPreference = 'Stop'

$identity = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $identity.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-ExePath', "`"$ExePath`"")
    if ($PrivateOnly) { $arguments += '-PrivateOnly' }
    if ($Remove) { $arguments += '-Remove' }
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $arguments -Wait
    exit 0
}

$group = 'OmniDrop'
Get-NetFirewallRule -Group $group -ErrorAction SilentlyContinue | Remove-NetFirewallRule
if ($Remove) {
    Write-Host 'OmniDrop firewall rules removed.'
    exit 0
}
if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "omnidrop.exe not found at '$ExePath'"
}
$profiles = if ($PrivateOnly) { @('Domain', 'Private') } else { @('Domain', 'Private', 'Public') }

New-NetFirewallRule -DisplayName 'OmniDrop - transfers (TCP)' -Group $group -Direction Inbound `
    -Program $ExePath -Protocol TCP -Action Allow -Profile $profiles `
    -Description 'Encrypted file transfers (port 44850, ephemeral fallback).' | Out-Null
New-NetFirewallRule -DisplayName 'OmniDrop - discovery (UDP)' -Group $group -Direction Inbound `
    -Program $ExePath -Protocol UDP -Action Allow -Profile $profiles `
    -Description 'mDNS (5353) and broadcast beacons (44851).' | Out-Null
New-NetFirewallRule -DisplayName 'OmniDrop - outbound' -Group $group -Direction Outbound `
    -Program $ExePath -Action Allow -Profile $profiles | Out-Null

Write-Host "OmniDrop firewall rules added for $ExePath ($($profiles -join ', '))."
