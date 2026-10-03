<#
.SYNOPSIS
    Installs Atrius, the code index, for the current user on Windows.

.DESCRIPTION
    Downloads the release archive for this machine, checks its SHA-256 against the release's
    SHA256SUMS, unpacks it into %LOCALAPPDATA%\Programs\atrius-index and puts that folder's bin on
    the user PATH. No administrator rights.

        irm https://blackrainbowlabs.com/downloads/atrius-index/install.ps1 | iex

    An earlier install's atrius processes (an index host, the background service) hold their
    programs open, so they are stopped first and the service, if it was installed, is started again
    from the new programs.

.PARAMETER Version
    A specific release to install. Defaults to the latest.

.PARAMETER BaseUrl
    Where the downloads are.

.PARAMETER FromDist
    Install the archive in dist/ instead of downloading.

.PARAMETER Prefix
    Where to install. Defaults to %LOCALAPPDATA%\Programs\atrius-index.

.PARAMETER NoPath
    Do not touch the user PATH.

.PARAMETER Uninstall
    Remove what this script installed, the background service and the PATH entry included.

.EXAMPLE
    pwsh packaging/install.ps1 -FromDist
    pwsh packaging/install.ps1 -Uninstall
#>
[CmdletBinding()]
param(
    [string] $Version,
    [string] $BaseUrl = 'https://blackrainbowlabs.com/downloads/atrius-index',
    [switch] $FromDist,
    [string] $Prefix,
    [switch] $NoPath,
    [switch] $Uninstall
)

$ErrorActionPreference = 'Stop'
if (-not $Prefix) { $Prefix = Join-Path $env:LOCALAPPDATA 'Programs\atrius-index' }
$binaries = Join-Path $Prefix 'bin'

function Add-ToUserPath {
    <#
    .SYNOPSIS
        Puts a folder on the user PATH, once, and on this session's PATH.
    .PARAMETER Directory
        The folder.
    #>
    param([string] $Directory)
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @()
    if ($current) { $parts = $current -split ';' | Where-Object { $_ -ne '' } }
    if ($parts -contains $Directory) { return }
    $parts += $Directory
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
    $env:Path = "$env:Path;$Directory"
    Write-Host "added $Directory to your PATH (new terminals will see it)"
}

function Remove-FromUserPath {
    <#
    .SYNOPSIS
        Takes a folder back off the user PATH.
    .PARAMETER Directory
        The folder.
    #>
    param([string] $Directory)
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $current) { return }
    $parts = $current -split ';' | Where-Object { $_ -ne '' -and $_ -ne $Directory }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
}

function Get-Architecture {
    <#
    .SYNOPSIS
        The Rust target triple this machine's releases are built for.
    #>
    switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
        'X64' { return 'x86_64-pc-windows-msvc' }
        default { throw "there is no Atrius release for $_ yet. Build from source: cargo install atrius-cli" }
    }
}

function Assert-Checksum {
    <#
    .SYNOPSIS
        Refuses an archive whose hash is not the one the release published.
    .PARAMETER Archive
        The downloaded file.
    .PARAMETER Sums
        The SHA256SUMS text from the same release.
    #>
    param([string] $Archive, [string] $Sums)
    $name = Split-Path -Leaf $Archive
    $expected = $null
    foreach ($line in ($Sums -split "`n")) {
        $fields = ($line.Trim() -split '\s+')
        if ($fields.Count -ge 2 -and $fields[-1] -eq $name) { $expected = $fields[0].ToLower() }
    }
    if (-not $expected) { throw "SHA256SUMS does not list $name" }
    $actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $expected) { throw "$name does not match its published checksum.`n  expected $expected`n  got      $actual" }
    Write-Host "checksum ok ($expected)"
}

function Stop-InstalledProcesses {
    <#
    .SYNOPSIS
        Stops every running program from this install, so its files can be replaced. Answers whether
        the background service was running.
    #>
    $running = @(Get-CimInstance Win32_Process -Filter "Name = 'atrius.exe' OR Name = 'atrius-mcp.exe'" |
        Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($binaries, [System.StringComparison]::OrdinalIgnoreCase) })
    $service = [bool]($running | Where-Object { $_.CommandLine -match '\sdaemon(\s|$)' })
    foreach ($process in $running) {
        Stop-Process -Id $process.ProcessId -Force -Confirm:$false -ErrorAction SilentlyContinue
    }
    if ($running.Count -gt 0) { Start-Sleep -Milliseconds 500 }
    return $service
}

if ($Uninstall) {
    $atrius = Join-Path $binaries 'atrius.exe'
    if (Test-Path -LiteralPath $atrius) { & $atrius service uninstall | Out-Null }
    [void](Stop-InstalledProcesses)
    if (Test-Path -LiteralPath $Prefix) {
        Remove-Item -LiteralPath $Prefix -Recurse -Force -Confirm:$false
        Write-Host "removed $Prefix"
    }
    Remove-FromUserPath -Directory $binaries
    Write-Host 'uninstalled. The indexes stay in %LOCALAPPDATA%\atrius-index and can be deleted.'
    return
}

$target = Get-Architecture
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ('atrius-install-' + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $temporary | Out-Null
try {
    if ($FromDist) {
        $dist = Join-Path (Split-Path -Parent $PSScriptRoot) 'dist'
        if (-not $Version) {
            $found = Get-ChildItem -Path $dist -Filter "atrius-index-*-$target.zip" | Select-Object -First 1
            if (-not $found) { throw "no archive for $target in $dist." }
            $Version = ($found.BaseName -replace '^atrius-index-', '') -replace "-$target$", ''
        }
        $archive = Join-Path $dist "atrius-index-$Version-$target.zip"
        if (-not (Test-Path -LiteralPath $archive)) { throw "$archive does not exist." }
        Assert-Checksum -Archive $archive -Sums (Get-Content -LiteralPath (Join-Path $dist 'SHA256SUMS') -Raw)
    } else {
        $base = $BaseUrl.TrimEnd('/')
        if (-not $Version) { $Version = ((Invoke-WebRequest -Uri "$base/VERSION" -UseBasicParsing).Content).Trim() }
        $archive = Join-Path $temporary "atrius-index-$Version-$target.zip"
        Write-Host "downloading Atrius $Version for $target..."
        Invoke-WebRequest -Uri "$base/atrius-index-$Version-$target.zip" -OutFile $archive -UseBasicParsing
        $sums = (Invoke-WebRequest -Uri "$base/SHA256SUMS" -UseBasicParsing).Content
        if ($sums -is [byte[]]) { $sums = [System.Text.Encoding]::UTF8.GetString($sums) }
        Assert-Checksum -Archive $archive -Sums $sums
    }

    $unpacked = Join-Path $temporary 'unpacked'
    Expand-Archive -LiteralPath $archive -DestinationPath $unpacked -Force
    $inner = Get-ChildItem -Path $unpacked -Directory | Select-Object -First 1
    if (-not $inner) { throw 'the archive holds no folder' }

    $serviceWasRunning = Stop-InstalledProcesses
    if (Test-Path -LiteralPath $Prefix) { Remove-Item -LiteralPath $Prefix -Recurse -Force -Confirm:$false }
    New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
    Copy-Item -Path (Join-Path $inner.FullName '*') -Destination $Prefix -Recurse -Force
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -Confirm:$false -ErrorAction SilentlyContinue
}

if (-not $NoPath) { Add-ToUserPath -Directory $binaries }
$atrius = Join-Path $binaries 'atrius.exe'
Write-Host ''
Write-Host "Atrius $Version is installed in $Prefix"
& $atrius --version
if ($serviceWasRunning) {
    & $atrius service install | Out-Null
    Write-Host 'the background service was running and has been started again'
}
Write-Host @"

Try, inside any git checkout:
  atrius find TODO
  atrius def main
  atrius doctor

Give an agent the same queries:
  claude mcp add atrius -- atrius-mcp

Keep a checkout indexed at all times:
  atrius watch add C:\code\my-project; atrius service install
"@
