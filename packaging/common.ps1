<#
.SYNOPSIS
    The helpers every release script dot sources: the version, the cross toolchain, the MSVC
    environment, what an archive holds, and SHA256SUMS.

.DESCRIPTION
    Adapted from Inillucent's packaging/stage-layout.ps1, which has made every Inillucent release
    since 0.1.3 from this Windows machine. Nothing here runs on import.

        . "$PSScriptRoot/common.ps1"
#>
$ErrorActionPreference = 'Stop'

# Git for Windows' GNU tar, which can set the modes NTFS cannot store.
$script:GnuTar = 'C:\Program Files\Git\usr\bin\tar.exe'

# The programs an archive carries.
$script:Programs = @('atrius', 'atrius-mcp')

function Get-WorkspaceVersion {
    <#
    .SYNOPSIS
        The version in the workspace manifest's [workspace.package].
    .PARAMETER Root
        The repository root.
    #>
    param([string] $Root)
    $manifest = Get-Content -Path (Join-Path $Root 'Cargo.toml') -Raw
    if ($manifest -match '(?ms)\[workspace\.package\].*?version\s*=\s*"([^"]+)"') { return $Matches[1] }
    throw 'Cargo.toml does not declare [workspace.package] version'
}

function Get-CrossBin {
    <#
    .SYNOPSIS
        Where zig, cargo-zigbuild, rcodesign, minisign and nfpm are.
    .DESCRIPTION
        ATRIUS_CROSS_BIN wins, then this checkout's tools/cross/bin, then the main checkout's when
        this is a git worktree, then INILLUCENT_CROSS_BIN or a sibling Inillucent checkout's, since
        both products are released from the same machine with the same pinned tools.
        `pwsh tools/cross/fetch-toolchain.ps1` fills this checkout's folder.
    .PARAMETER Root
        The repository root.
    #>
    param([string] $Root)
    $candidates = @()
    if ($env:ATRIUS_CROSS_BIN) { $candidates += $env:ATRIUS_CROSS_BIN }
    $candidates += Join-Path $Root 'tools/cross/bin'
    $commonDir = (& git -C $Root rev-parse --git-common-dir 2>$null)
    if ($commonDir) {
        $resolved = if ([System.IO.Path]::IsPathRooted($commonDir)) { $commonDir } else { Join-Path $Root $commonDir }
        $candidates += Join-Path (Split-Path -Parent ([System.IO.Path]::GetFullPath($resolved))) 'tools/cross/bin'
    }
    if ($env:INILLUCENT_CROSS_BIN) { $candidates += $env:INILLUCENT_CROSS_BIN }
    $candidates += Join-Path (Split-Path -Parent $Root) 'inillucent/tools/cross/bin'
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath (Join-Path $candidate 'cargo-zigbuild.exe')) { return $candidate }
    }
    return (Join-Path $Root 'tools/cross/bin')
}

function Import-MsvcEnvironment {
    <#
    .SYNOPSIS
        Puts the MSVC compiler's environment into this process when INCLUDE is not set, so
        `onig_sys` can compile its C code.
    #>
    if ($env:INCLUDE) { return }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { throw "INCLUDE is not set and $vswhere does not exist." }
    $install = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Select-Object -First 1)
    if (-not $install) { throw 'vswhere found no Visual Studio with the C++ tools installed.' }
    $vcvars = Join-Path $install 'VC\Auxiliary\Build\vcvars64.bat'
    & cmd /c "`"$vcvars`" >nul 2>&1 && set" | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "env:$($Matches[1])" -Value $Matches[2] }
    }
    if (-not $env:INCLUDE) { throw "running $vcvars did not set INCLUDE." }
}

function New-Stage {
    <#
    .SYNOPSIS
        Builds the folder an archive is made from, and returns its path: bin/ with the two programs,
        and the README, licences, CHANGELOG, docs and VERSION.
    .PARAMETER Root
        The repository root.
    .PARAMETER Version
        The version.
    .PARAMETER Target
        The target triple, which names the archive.
    .PARAMETER BuiltDir
        The folder the programs were built into.
    .PARAMETER Dist
        Where the staged folder goes.
    #>
    param([string] $Root, [string] $Version, [string] $Target, [string] $BuiltDir, [string] $Dist)
    $exe = if ($Target -like '*windows*') { '.exe' } else { '' }
    $stage = Join-Path $Dist "atrius-index-$Version-$Target"
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force -Confirm:$false }
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'bin') | Out-Null
    foreach ($program in $script:Programs) {
        $from = Join-Path $BuiltDir "$program$exe"
        if (-not (Test-Path -LiteralPath $from)) { throw "the build did not produce $from" }
        Copy-Item -LiteralPath $from -Destination (Join-Path $stage 'bin') -Force
    }
    foreach ($file in @('README.md', 'CHANGELOG.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'SECURITY.md')) {
        Copy-Item -LiteralPath (Join-Path $Root $file) -Destination $stage -Force
    }
    Copy-Item -LiteralPath (Join-Path $Root 'docs') -Destination (Join-Path $stage 'docs') -Recurse -Force
    Set-Content -Path (Join-Path $stage 'VERSION') -Value $Version -NoNewline
    return $stage
}

function New-TarGz {
    <#
    .SYNOPSIS
        Writes the .tar.gz of a staged folder, with bin/ at mode 0755 and everything else at 0644.
    .DESCRIPTION
        NTFS has no execute bit, so the modes are set by tar in two passes rather than read from
        disk: everything readable, then bin/ appended at 0755.
    .PARAMETER Stage
        The staged folder.
    .PARAMETER Dist
        Where the archive goes.
    #>
    param([string] $Stage, [string] $Dist)
    if (-not (Test-Path -LiteralPath $script:GnuTar)) { throw "GNU tar is not at $($script:GnuTar). Install Git for Windows." }
    $name = Split-Path -Leaf $Stage
    $tar = Join-Path $Dist "$name.tar"
    $archive = "$tar.gz"
    foreach ($stale in @($tar, $archive)) { if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Force -Confirm:$false } }
    $tarArgument = $tar -replace '\\', '/'
    $distArgument = $Dist -replace '\\', '/'
    $common = @('--force-local', '--owner=0', '--group=0', '--numeric-owner', '--sort=name')
    & $script:GnuTar @common '--mode=a+rX,u+w' "--exclude=$name/bin" -cf $tarArgument -C $distArgument $name
    if ($LASTEXITCODE -ne 0) { throw "tar failed with $LASTEXITCODE" }
    & $script:GnuTar @common '--mode=0755' -rf $tarArgument -C $distArgument "$name/bin"
    if ($LASTEXITCODE -ne 0) { throw "tar append failed with $LASTEXITCODE" }
    $input = [System.IO.File]::OpenRead($tar)
    try {
        $output = [System.IO.File]::Create($archive)
        try {
            $gzip = New-Object System.IO.Compression.GZipStream($output, [System.IO.Compression.CompressionLevel]::Optimal)
            try { $input.CopyTo($gzip) } finally { $gzip.Dispose() }
        } finally { $output.Dispose() }
    } finally { $input.Dispose() }
    Remove-Item -LiteralPath $tar -Force -Confirm:$false
    return $archive
}

function New-Zip {
    <#
    .SYNOPSIS
        Writes the .zip of a staged folder.
    .PARAMETER Stage
        The staged folder.
    .PARAMETER Dist
        Where the archive goes.
    #>
    param([string] $Stage, [string] $Dist)
    $archive = Join-Path $Dist ((Split-Path -Leaf $Stage) + '.zip')
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force -Confirm:$false }
    Compress-Archive -Path $Stage -DestinationPath $archive -CompressionLevel Optimal
    return $archive
}

function Update-Sha256Sums {
    <#
    .SYNOPSIS
        Rewrites dist/SHA256SUMS over this release's artifacts, with LF line endings.
    .DESCRIPTION
        Every installer verifies against exactly this file, and the site publishes it as it is. Only
        this version's files are named, because dist/ is not emptied between releases, and the macOS
        zip is left out because it is the notary's container rather than a download.
    .PARAMETER Dist
        The dist folder.
    .PARAMETER Version
        The version.
    #>
    param([string] $Dist, [string] $Version)
    $lines = @()
    foreach ($pattern in @('*.zip', '*.tar.gz', '*.deb', '*.rpm', '*.pkg')) {
        Get-ChildItem -Path $Dist -Filter $pattern -File -ErrorAction SilentlyContinue | Sort-Object Name | ForEach-Object {
            if ($_.Name -like "*$Version*" -and $_.Name -notlike '*-apple-darwin.zip') {
                $lines += "$((Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLower())  $($_.Name)"
            }
        }
    }
    $sums = Join-Path $Dist 'SHA256SUMS'
    $text = ($lines -join "`n")
    if ($lines.Count -gt 0) { $text += "`n" }
    [System.IO.File]::WriteAllText($sums, $text, (New-Object System.Text.UTF8Encoding($false)))
    return $sums
}
