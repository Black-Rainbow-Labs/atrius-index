<#
.SYNOPSIS
    Runs cargo with the MSVC compiler's environment, on Windows.

.DESCRIPTION
    `inillucent-driver` with `embed` depends on `onig_sys`, which compiles oniguruma with `cl.exe`.
    cl.exe finds its headers through INCLUDE, LIB and PATH, which `vcvars64.bat` sets. A developer
    shell has run it; an agent terminal, a scheduled task and a plain PowerShell have not, and the
    build then stops at `fatal error C1083: Cannot open include file: 'stddef.h'`.

    This script finds Visual Studio with vswhere, copies the environment vcvars64.bat produces into
    this process when INCLUDE is not set, and runs cargo with the arguments it was given.

.EXAMPLE
    pwsh tools/cargo.ps1 test --workspace
#>
$ErrorActionPreference = 'Stop'

function Import-MsvcEnvironment {
    <#
    .SYNOPSIS
        Puts the MSVC compiler's own environment variables into this process, if they are missing.
    #>
    if ($env:INCLUDE -or -not $IsWindows) { return }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw "INCLUDE is not set and $vswhere does not exist. Build from a Developer PowerShell, or install Visual Studio's C++ tools."
    }
    $install = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
    if (-not $install) { throw 'vswhere found no Visual Studio with the C++ tools installed.' }
    $vcvars = Join-Path $install 'VC\Auxiliary\Build\vcvars64.bat'
    if (-not (Test-Path -LiteralPath $vcvars)) { throw "$vcvars does not exist." }
    & cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
    }
    if (-not $env:INCLUDE) { throw "running $vcvars did not set INCLUDE." }
}

Import-MsvcEnvironment
$manifest = Join-Path (Split-Path -Parent $PSScriptRoot) 'Cargo.toml'
if ($args -notcontains '--manifest-path' -and $args.Count -gt 0 -and $args[0] -notin @('--version', 'fmt')) {
    $rest = @($args | Select-Object -Skip 1)
    & cargo $args[0] --manifest-path $manifest @rest
} else {
    & cargo @args
}
exit $LASTEXITCODE
