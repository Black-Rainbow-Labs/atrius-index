<#
.SYNOPSIS
    Builds Atrius for every published target on this Windows machine, and writes the Windows and
    Linux archives.

.DESCRIPTION
    | Target                      | Built with                       | Archive              |
    |-----------------------------|----------------------------------|----------------------|
    | x86_64-pc-windows-msvc      | cargo, with the MSVC environment | .zip                 |
    | x86_64-unknown-linux-gnu    | cargo-zigbuild, glibc 2.28 floor | .tar.gz              |
    | aarch64-unknown-linux-gnu   | cargo-zigbuild, glibc 2.28 floor | .tar.gz              |
    | aarch64-apple-darwin        | cargo-zigbuild                   | (macos/release-macos.ps1) |
    | x86_64-apple-darwin         | cargo-zigbuild                   | (macos/release-macos.ps1) |

    Each target builds in its own target folder, so the five run at once; cargo locks a target
    folder, and five builds in one would run one at a time. The macOS archives and the .pkg are
    written by packaging/macos/release-macos.ps1, because they are made from signed binaries.

.PARAMETER Version
    The version the archives are named for. Defaults to the workspace version.

.PARAMETER Targets
    `all`, `windows`, `linux` or `macos`.

.PARAMETER TargetRoot
    Where the per target folders go. Defaults to CARGO_TARGET_DIR, else target/, plus `release-all`.

.EXAMPLE
    pwsh packaging/build.ps1
    pwsh packaging/build.ps1 -Targets windows
#>
[CmdletBinding()]
param(
    [string] $Version,
    [ValidateSet('all', 'windows', 'linux', 'macos')] [string] $Targets = 'all',
    [string] $TargetRoot
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
. (Join-Path $PSScriptRoot 'common.ps1')
if (-not $Version) { $Version = Get-WorkspaceVersion -Root $root }
if (-not $TargetRoot) {
    $base = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $TargetRoot = Join-Path $base 'release-all'
}
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist, $TargetRoot | Out-Null
$crossBin = Get-CrossBin -Root $root
$zigbuild = Join-Path $crossBin 'cargo-zigbuild.exe'
$zigDir = Join-Path $crossBin 'zig'
$manifest = Join-Path $root 'Cargo.toml'

$plan = @(
    @{ Triple = 'x86_64-pc-windows-msvc'; Group = 'windows'; Zig = $false; Build = 'x86_64-pc-windows-msvc' },
    @{ Triple = 'x86_64-unknown-linux-gnu'; Group = 'linux'; Zig = $true; Build = 'x86_64-unknown-linux-gnu.2.28' },
    @{ Triple = 'aarch64-unknown-linux-gnu'; Group = 'linux'; Zig = $true; Build = 'aarch64-unknown-linux-gnu.2.28' },
    @{ Triple = 'aarch64-apple-darwin'; Group = 'macos'; Zig = $true; Build = 'aarch64-apple-darwin' },
    @{ Triple = 'x86_64-apple-darwin'; Group = 'macos'; Zig = $true; Build = 'x86_64-apple-darwin' }
) | Where-Object { $Targets -eq 'all' -or $_.Group -eq $Targets }

if (($plan | Where-Object Zig) -and -not (Test-Path -LiteralPath $zigbuild)) {
    throw "cargo-zigbuild is not in $crossBin. Run: pwsh tools/cross/fetch-toolchain.ps1"
}
Import-MsvcEnvironment
$previousPath = $env:PATH
$env:PATH = "$zigDir;$env:PATH"
$running = @()
try {
    foreach ($entry in $plan) {
        $targetDir = Join-Path $TargetRoot $entry.Triple
        $log = Join-Path $TargetRoot "$($entry.Triple).log"
        $arguments = @()
        if ($entry.Zig) {
            $program = $zigbuild
            $arguments += 'zigbuild'
        } else {
            $program = 'cargo'
            $arguments += 'build'
        }
        $arguments += @('--manifest-path', $manifest, '--release', '--locked', '--target', $entry.Build, '--target-dir', $targetDir, '-p', 'atrius-cli')
        if ($entry.Triple -like '*apple*') {
            # Room for the code signature load command; without it rcodesign refuses the x86-64 slice.
            $arguments += @('--config', "target.$($entry.Triple).rustflags=[`"-C`",`"link-arg=-Wl,-headerpad_max_install_names`"]")
        }
        Write-Host "== building $($entry.Triple)"
        $process = Start-Process -FilePath $program -ArgumentList $arguments -NoNewWindow -PassThru `
            -RedirectStandardError $log -RedirectStandardOutput "$log.out"
        $running += @{ Entry = $entry; Process = $process; Log = $log; TargetDir = $targetDir }
    }
    foreach ($job in $running) {
        $job.Process.WaitForExit()
        if ($job.Process.ExitCode -ne 0) {
            Get-Content -LiteralPath $job.Log -Tail 30 | Write-Host
            throw "the build for $($job.Entry.Triple) failed with $($job.Process.ExitCode); the log is $($job.Log)"
        }
        Write-Host "   built $($job.Entry.Triple)"
    }
} finally {
    $env:PATH = $previousPath
}

foreach ($job in $running) {
    $triple = $job.Entry.Triple
    if ($triple -like '*apple*') { continue }
    $built = Join-Path $job.TargetDir "$triple/release"
    $stage = New-Stage -Root $root -Version $Version -Target $triple -BuiltDir $built -Dist $dist
    $archive = if ($triple -like '*windows*') { New-Zip -Stage $stage -Dist $dist } else { New-TarGz -Stage $stage -Dist $dist }
    Write-Host "   $archive"
}
$sums = Update-Sha256Sums -Dist $dist -Version $Version
Write-Host "sums $sums"
