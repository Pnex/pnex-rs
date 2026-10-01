# PNeX edge agent installer (Windows) — served by pnex-server at
# /api/v1/agent/install.ps1 (D95, docs/architecture/edge-agent.md).
#
# Run from an elevated PowerShell (5.1 or 7). The UI prints a one-liner that
# first pins the server CA by its SHA-256, then invokes this script with
# -Server, -Enroll and -CaSha256. Extra arguments are forwarded to
# `pnex-agent install` (e.g. -Extra '--listen','0.0.0.0:7070').
param(
    [Parameter(Mandatory = $true)][string]$Server,
    [Parameter(Mandatory = $true)][string]$Enroll,
    [string]$CaSha256 = "",
    [string[]]$Extra = @()
)
$ErrorActionPreference = "Stop"
$Server = $Server.TrimEnd('/')

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "pnex-agent install: run this from an elevated (Administrator) PowerShell"
}
if ($env:PROCESSOR_ARCHITECTURE -ne "AMD64") {
    throw "pnex-agent install: unsupported architecture $($env:PROCESSOR_ARCHITECTURE) (x86_64 only)"
}

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
function Get-Sha256Hex([byte[]]$Bytes) {
    $sha = [Security.Cryptography.SHA256]::Create()
    return ([BitConverter]::ToString($sha.ComputeHash($Bytes)) -replace '-', '').ToLower()
}

if ($CaSha256 -ne "") {
    # Pin the server CA (trust on first use, fingerprint from the UI).
    [Net.ServicePointManager]::ServerCertificateValidationCallback = { $true }
    $caBytes = (New-Object Net.WebClient).DownloadData("$Server/api/v1/meta/ca")
    if ((Get-Sha256Hex $caBytes) -ne $CaSha256.ToLower()) { throw "pnex-agent install: CA fingerprint mismatch" }
    $global:PnexCa = New-Object Security.Cryptography.X509Certificates.X509Certificate2(, $caBytes)
    [Net.ServicePointManager]::ServerCertificateValidationCallback = {
        param($sender, $cert, $chain, $errors)
        $x = New-Object Security.Cryptography.X509Certificates.X509Chain
        $x.ChainPolicy.ExtraStore.Add($global:PnexCa) | Out-Null
        $x.ChainPolicy.RevocationMode = "NoCheck"
        $x.ChainPolicy.VerificationFlags = "AllowUnknownCertificateAuthority"
        $null = $x.Build($cert)
        $root = $x.ChainElements[$x.ChainElements.Count - 1].Certificate
        return $root.Thumbprint -eq $global:PnexCa.Thumbprint
    }
}

$file = "pnex-agent-x86_64-windows.exe"
$web = New-Object Net.WebClient
Write-Host "Downloading $file from $Server ..."
$bin = $web.DownloadData("$Server/api/v1/agent/download/x86_64-windows")
$sums = $web.DownloadString("$Server/api/v1/agent/download/SHA256SUMS")
$want = ($sums -split "`n" | Where-Object { $_ -match " $([regex]::Escape($file))\s*$" } | ForEach-Object { ($_ -split ' ')[0] })
if (-not $want) { throw "pnex-agent install: no checksum published for $file" }
if ((Get-Sha256Hex $bin) -ne $want.Trim()) { throw "pnex-agent install: binary checksum mismatch" }

$dir = Join-Path $env:ProgramFiles "PNeX"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$exe = Join-Path $dir "pnex-agent.exe"
# Replacing a running service binary fails: stop it first on reinstall.
if (Get-Service -Name "pnex-agent" -ErrorAction SilentlyContinue) { Stop-Service -Name "pnex-agent" -Force -ErrorAction SilentlyContinue }
[IO.File]::WriteAllBytes($exe, $bin)
Write-Host "Installed $exe"

$args = @("install", "--server", $Server, "--enroll", $Enroll)
if ($CaSha256 -ne "") { $args += @("--ca-sha256", $CaSha256) }
$args += $Extra
& $exe @args
exit $LASTEXITCODE
