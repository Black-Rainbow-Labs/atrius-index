<#
.SYNOPSIS
    The macOS release on the Windows machine: universal binaries, Developer ID signatures, the two
    archives and the .pkg, notarized by Apple.

.DESCRIPTION
    Adapted from Inillucent's packaging/macos/release-macos.ps1, which has shipped notarized releases
    this way. No Mac is involved:

        lipo         -> rcodesign macho-universal-create
        codesign     -> rcodesign sign
        pkgbuild     -> tools/macos-pkg
        productsign  -> rcodesign sign
        notarytool   -> rcodesign notary-submit
        stapler      -> rcodesign notary-submit --staple

    Writes:

        dist/atrius-index-<version>-universal-apple-darwin.tar.gz   install.sh and Homebrew
        dist/atrius-index-<version>-universal-apple-darwin.zip      the notary submission only
        dist/atrius-index-<version>.pkg                             the double click installer

    The credentials are Black Rainbow Labs' Developer ID certificates and App Store Connect key,
    sealed with DPAPI in %LOCALAPPDATA%\inillucent\apple and read by apple-credentials.ps1. Both
    products are signed by the same team.

    Both programs are signed with entitlements.plist, because `embed()` loads ONNX Runtime, a library
    signed by another team, which the hardened runtime refuses without it.

.PARAMETER Version
    The release version. Defaults to the workspace version.

.PARAMETER BuiltRoot
    The folder holding one target folder per Apple triple, as packaging/build.ps1 leaves them.

.PARAMETER SkipNotarize
    Sign and package, and submit nothing to Apple.

.PARAMETER SelfSigned
    Sign with a certificate made on the spot, to prove every step without the real certificates.
    Writes to dist/unpublishable.
#>
[CmdletBinding()]
param(
    [string] $Version,
    [string] $BuiltRoot,
    [switch] $SkipNotarize,
    [switch] $SelfSigned,
    [string] $Identifier = 'com.blackrainbowlabs.atrius-index'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path (Split-Path -Parent $PSScriptRoot) 'common.ps1')
. (Join-Path $PSScriptRoot 'apple-credentials.ps1')

$crossBin = Get-CrossBin -Root $root
$rcodesign = Join-Path $crossBin 'rcodesign.exe'
$entitlements = Join-Path $PSScriptRoot 'entitlements.plist'
$expectedMinimumOs = '13.0.0'
if (-not $Version) { $Version = Get-WorkspaceVersion -Root $root }
if (-not $BuiltRoot) {
    $base = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $BuiltRoot = Join-Path $base 'release-all'
}
if ($SelfSigned -and -not $SkipNotarize) { throw 'a self signed certificate cannot be notarized; add -SkipNotarize' }

function Invoke-Rcodesign {
    <#
    .SYNOPSIS
        Runs rcodesign and throws when it fails.
    .PARAMETER Arguments
        The command line.
    .PARAMETER WorkingDirectory
        Where it runs. rcodesign reads the text before a colon in some arguments as a scope, so a file
        with a drive letter is named from its own folder.
    #>
    param([string[]] $Arguments, [string] $WorkingDirectory)
    $out = [System.IO.Path]::GetTempFileName()
    $err = [System.IO.Path]::GetTempFileName()
    try {
        $where = if ($WorkingDirectory) { $WorkingDirectory } else { (Get-Location).Path }
        $process = Start-Process -FilePath $rcodesign -ArgumentList $Arguments -WorkingDirectory $where `
            -Wait -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
        if ($process.ExitCode -ne 0) { throw "rcodesign $($Arguments[0]) failed with $($process.ExitCode): $(Get-Content -LiteralPath $err -Raw)" }
        return (Get-Content -LiteralPath $out, $err)
    } finally {
        Remove-Item -LiteralPath $out, $err -Force -Confirm:$false -ErrorAction SilentlyContinue
    }
}

function Assert-SignedUniversal {
    <#
    .SYNOPSIS
        Fails unless a binary holds both slices, each declaring the minimum macOS this release states,
        each signed with the hardened runtime, the entitlement and a trusted timestamp.
    .PARAMETER Path
        The binary.
    #>
    param([string] $Path)
    foreach ($index in 0, 1) {
        $target = & $rcodesign extract macho-target --universal-index $index $Path 2>&1
        if ($LASTEXITCODE -ne 0) { throw "$Path has no slice at index $index" }
        $found = ($target | Select-String -Pattern 'Minimum OS: (\S+)').Matches.Groups[1].Value
        if ($found -ne $expectedMinimumOs) { throw "$Path slice $index declares macOS $found; this release states $expectedMinimumOs" }
    }
    $info = & $rcodesign print-signature-info $Path 2>&1
    if (($info | Select-String -Pattern 'CodeSignatureFlags\(RUNTIME\)').Count -lt 2) { throw "$Path: not every slice has the hardened runtime" }
    if (($info | Select-String -Pattern 'com\.apple\.security\.cs\.disable-library-validation').Count -lt 4) { throw "$Path: not every slice carries the entitlement" }
    if (-not ($info | Select-String -Pattern 'time_stamp_token')) { throw "$Path carries no trusted timestamp" }
}

function New-SelfSignedIdentity {
    <#
    .SYNOPSIS
        A throwaway certificate on the RAM disk, for proving the steps.
    .PARAMETER Kind
        `application` or `installer`.
    #>
    param([string] $Kind)
    $pem = Join-Path (Get-AppleScratchDir) ("selfsigned-$Kind-" + [guid]::NewGuid().ToString('N') + '.pem')
    [void](Invoke-Rcodesign -Arguments @('generate-self-signed-certificate', '--algorithm', 'rsa', '--profile', "developer-id-$Kind", '--team-id', 'SELFSIGNED', '--person-name', 'atrius self signed proof', '--validity-days', '30', '--pem-unified-file', $pem))
    return [pscustomobject]@{ Arguments = @('--pem-file', $pem); Scratch = @($pem) }
}

$dist = if ($SelfSigned) { Join-Path $root 'dist/unpublishable' } else { Join-Path $root 'dist' }
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Write-Host "Atrius $Version: macOS, signed on Windows"

if (-not $SelfSigned) {
    $present = Test-AppleCredentials
    foreach ($needed in @('Application', 'Installer')) {
        if (-not $present.$needed) { throw "no Developer ID $needed certificate in $($present.Directory)" }
    }
    if (-not $SkipNotarize -and -not $present.NotaryKey) { throw "no notary key in $($present.Directory)" }
}

# The package writer, built once into a folder of its own.
$pkgTargetDir = Join-Path (Split-Path -Parent $BuiltRoot) 'macos-pkg'
$pkgTool = Join-Path $pkgTargetDir 'release/macos-pkg.exe'
if (-not (Test-Path -LiteralPath $pkgTool)) {
    & cargo build --manifest-path (Join-Path $root 'tools/macos-pkg/Cargo.toml') --release --target-dir $pkgTargetDir
    if ($LASTEXITCODE -ne 0) { throw "building macos-pkg failed with $LASTEXITCODE" }
}

$universal = Join-Path $dist 'universal-apple-darwin'
if (Test-Path -LiteralPath $universal) { Remove-Item -LiteralPath $universal -Recurse -Force -Confirm:$false }
New-Item -ItemType Directory -Force -Path $universal | Out-Null
foreach ($program in $script:Programs) {
    $arm = Join-Path $BuiltRoot "aarch64-apple-darwin/aarch64-apple-darwin/release/$program"
    $intel = Join-Path $BuiltRoot "x86_64-apple-darwin/x86_64-apple-darwin/release/$program"
    foreach ($input in @($arm, $intel)) { if (-not (Test-Path -LiteralPath $input)) { throw "the build did not produce $input. Run: pwsh packaging/build.ps1 -Targets macos" } }
    [void](Invoke-Rcodesign -Arguments @('macho-universal-create', '--output', (Join-Path $universal $program), $arm, $intel))
}

$application = $null
$installer = $null
try {
    $application = if ($SelfSigned) { New-SelfSignedIdentity -Kind application } else { New-AppleSigningSession -Kind application }
    Write-Host '== signing'
    foreach ($program in $script:Programs) {
        $path = Join-Path $universal $program
        [void](Invoke-Rcodesign -WorkingDirectory $PSScriptRoot -Arguments (@('sign') + $application.Arguments + @(
                    '--binary-identifier', $program, '--code-signature-flags', 'runtime',
                    '--entitlements-xml-file', (Split-Path -Leaf $entitlements), $path)))
        Assert-SignedUniversal -Path $path
        Write-Host "   signed $program"
    }

    Write-Host '== archives'
    $stage = New-Stage -Root $root -Version $Version -Target 'universal-apple-darwin' -BuiltDir $universal -Dist $dist
    $tarball = New-TarGz -Stage $stage -Dist $dist
    $zip = New-Zip -Stage $stage -Dist $dist
    Write-Host "   $tarball"

    Write-Host '== package'
    $installer = if ($SelfSigned) { New-SelfSignedIdentity -Kind installer } else { New-AppleSigningSession -Kind installer }
    $pkgroot = Join-Path $dist 'pkgroot'
    if (Test-Path -LiteralPath $pkgroot) { Remove-Item -LiteralPath $pkgroot -Recurse -Force -Confirm:$false }
    New-Item -ItemType Directory -Force -Path (Join-Path $pkgroot 'usr/local/bin') | Out-Null
    Copy-Item -Path (Join-Path $stage 'bin/*') -Destination (Join-Path $pkgroot 'usr/local/bin') -Force
    $distribution = Join-Path $dist 'Distribution.xml'
    (Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Distribution.xml') -Raw).Replace('@VERSION@', $Version) |
        Set-Content -LiteralPath $distribution -NoNewline
    $licence = Join-Path $dist 'LICENSE'
    Copy-Item -LiteralPath (Join-Path $root 'LICENSE-MIT') -Destination $licence -Force
    $unsigned = Join-Path $dist "atrius-index-$Version-unsigned.pkg"
    $product = Join-Path $dist "atrius-index-$Version.pkg"
    foreach ($stale in @($unsigned, $product)) { if (Test-Path -LiteralPath $stale) { Remove-Item -LiteralPath $stale -Force -Confirm:$false } }
    & $pkgTool --root $pkgroot --identifier $Identifier --version $Version --install-location / `
        --executable-dir usr/local/bin --distribution $distribution --component-name "atrius-index-component-$Version.pkg" `
        --resource (Join-Path $PSScriptRoot 'welcome.txt') --resource (Join-Path $PSScriptRoot 'conclusion.txt') `
        --resource $licence --output $unsigned | Write-Host
    if ($LASTEXITCODE -ne 0) { throw "macos-pkg failed with $LASTEXITCODE" }
    [void](Invoke-Rcodesign -Arguments (@('sign') + $installer.Arguments + @($unsigned, $product)))
    Remove-Item -LiteralPath $unsigned, $distribution, $licence -Force -Confirm:$false
    Remove-Item -LiteralPath $pkgroot -Recurse -Force -Confirm:$false
    Write-Host "   $product"
} finally {
    foreach ($identity in @($application, $installer)) {
        if (-not $identity) { continue }
        if ($SelfSigned) {
            foreach ($path in $identity.Scratch) { Remove-Item -LiteralPath $path -Force -Confirm:$false -ErrorAction SilentlyContinue }
        } else {
            Remove-AppleSigningSession -Session $identity
        }
    }
}

if (-not $SkipNotarize) {
    Write-Host '== notarizing; Apple usually answers in two to fifteen minutes'
    $notary = $null
    try {
        $notary = New-AppleNotarySession
        Invoke-Rcodesign -Arguments @('notary-submit', '--api-key-file', $notary.Path, '--wait', '--staple', $product) | Write-Host
        Invoke-Rcodesign -Arguments @('notary-submit', '--api-key-file', $notary.Path, '--wait', $zip) | Write-Host
    } finally {
        Remove-AppleNotarySession -Session $notary
    }
}

Remove-Item -LiteralPath $universal -Recurse -Force -Confirm:$false
$sums = Update-Sha256Sums -Dist $dist -Version $Version
Write-Host "sums $sums"
Write-Host 'checked here: both slices in each program, macOS 13.0 minimum, hardened runtime, entitlement, timestamp'
if ($SkipNotarize) { Write-Host 'NOT notarized (-SkipNotarize), so nothing here is publishable' } else { Write-Host 'Apple notarized the .pkg and the .zip; the .pkg carries a stapled ticket' }
