<#
.SYNOPSIS
    Builds the .deb and .rpm for amd64 and arm64 from the staged Linux archives, on Windows.

.DESCRIPTION
    nfpm writes both formats and signs both with an OpenPGP key, from the same staged folders the
    .tar.gz archives are made from. The key is Black Rainbow Labs' package signing key, which also
    signs Inillucent's packages: its id is in INILLUCENT_GPG_KEY (or `gpg.keyid` in the sealed store)
    and its passphrase in INILLUCENT_GPG_PASSPHRASE. nfpm reads the key from a file, so it is exported
    to the RAM disk for the run and deleted in a finally.

.PARAMETER Version
    The version. Defaults to the workspace version.

.PARAMETER Unsigned
    Skip the OpenPGP signature.
#>
[CmdletBinding()]
param(
    [string] $Version,
    [switch] $Unsigned,
    [string] $GpgKey = $env:INILLUCENT_GPG_KEY,
    [string] $GpgPassphraseEnv = 'INILLUCENT_GPG_PASSPHRASE'
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
. (Join-Path $root 'packaging/common.ps1')
$nfpm = Join-Path (Get-CrossBin -Root $root) 'nfpm.exe'
if (-not (Test-Path -LiteralPath $nfpm)) { throw 'nfpm is missing. Run: pwsh tools/cross/fetch-toolchain.ps1' }
$gpg = 'C:\Program Files\Git\usr\bin\gpg.exe'
if (-not $Version) { $Version = Get-WorkspaceVersion -Root $root }
$dist = Join-Path $root 'dist'
$work = Join-Path $dist '_nfpm'
New-Item -ItemType Directory -Force -Path $work | Out-Null

$architectures = @(
    @{ Triple = 'x86_64-unknown-linux-gnu'; Nfpm = 'amd64'; Deb = 'amd64'; Rpm = 'x86_64' },
    @{ Triple = 'aarch64-unknown-linux-gnu'; Nfpm = 'arm64'; Deb = 'arm64'; Rpm = 'aarch64' }
)

$keyFile = $null
$previousPath = $env:Path
try {
    $signatureBlock = ''
    if (-not $Unsigned) {
        if (-not $GpgKey) { throw 'no signing key. Set INILLUCENT_GPG_KEY, pass -GpgKey, or pass -Unsigned.' }
        $passphrase = [System.Environment]::GetEnvironmentVariable($GpgPassphraseEnv)
        if (-not $passphrase) { throw "$GpgPassphraseEnv is not set" }
        # Git's usr\bin goes first so gpg can start keyboxd; the PATH is restored in the finally,
        # because Git's link.exe would otherwise shadow MSVC's for whatever runs next.
        $env:Path = "$(Split-Path -Parent $gpg);$env:Path"
        $scratch = if (Test-Path -LiteralPath 'R:\') { 'R:\atrius-release' } else { [System.IO.Path]::GetTempPath() }
        New-Item -ItemType Directory -Force -Path $scratch | Out-Null
        $stamp = [guid]::NewGuid().ToString('N')
        $keyFile = Join-Path $scratch "signing-$stamp.asc"
        $passphraseFile = Join-Path $scratch "passphrase-$stamp.txt"
        try {
            Set-Content -Path $passphraseFile -Value $passphrase -NoNewline -Encoding ascii
            & $gpg --batch --yes --pinentry-mode loopback --passphrase-file $passphraseFile --armor --output $keyFile --export-secret-keys $GpgKey
            if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $keyFile)) { throw "gpg could not export $GpgKey" }
        } finally {
            Remove-Item -LiteralPath $passphraseFile -Force -Confirm:$false -ErrorAction SilentlyContinue
        }
        $env:NFPM_PASSPHRASE = $passphrase
        $escaped = ($keyFile -replace '\\', '/')
        $signatureBlock = "`ndeb:`n  signature:`n    key_file: $escaped`n    type: origin`nrpm:`n  signature:`n    key_file: $escaped`n"
    }
    $template = Get-Content -Path (Join-Path $PSScriptRoot 'nfpm.template.yaml') -Raw
    foreach ($architecture in $architectures) {
        $stage = Join-Path $dist "atrius-index-$Version-$($architecture.Triple)"
        if (-not (Test-Path -LiteralPath $stage)) { throw "$stage does not exist. Run: pwsh packaging/build.ps1 -Targets linux" }
        $config = Join-Path $work "$($architecture.Nfpm).yaml"
        $filled = $template.Replace('@VERSION@', $Version).Replace('@ARCH@', $architecture.Nfpm).Replace('@STAGE@', ($stage -replace '\\', '/')) + $signatureBlock
        Set-Content -Path $config -Value $filled -Encoding utf8
        foreach ($format in @('deb', 'rpm')) {
            $target = if ($format -eq 'deb') { Join-Path $dist "atrius-index_${Version}_$($architecture.Deb).deb" } else { Join-Path $dist "atrius-index-$Version.$($architecture.Rpm).rpm" }
            & $nfpm package --config $config --packager $format --target $target | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "nfpm failed for $format/$($architecture.Nfpm) with $LASTEXITCODE" }
            Write-Host "  $target"
        }
    }
} finally {
    $env:Path = $previousPath
    if ($keyFile -and (Test-Path -LiteralPath $keyFile)) { Remove-Item -LiteralPath $keyFile -Force -Confirm:$false }
    if ($env:NFPM_PASSPHRASE) { Remove-Item Env:\NFPM_PASSPHRASE }
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force -Confirm:$false }
}
$sums = Update-Sha256Sums -Dist $dist -Version $Version
Write-Host "sums $sums"
