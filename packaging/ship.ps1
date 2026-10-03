<#
.SYNOPSIS
    Releases Atrius in one command: version, build, sign, publish, and then ask every destination
    what it serves.

.DESCRIPTION
    The same five phases as Inillucent's packaging/ship.ps1, cut down to Atrius's routes.

    | Phase     | What it does |
    |-----------|--------------|
    | preflight | reads every credential and tool and prints which routes run, and why any will not. Changes nothing |
    | version   | writes the version into Cargo.toml (twice) and Cargo.lock |
    | routes    | each route below, in order; one failing does not stop the rest |
    | report    | one line per route, and exit 1 when any failed |

    | Route    | Needs | Publishes |
    |----------|-------|-----------|
    | build    | cargo-zigbuild and zig | the Windows zip and the four Linux and macOS builds |
    | macos    | the Developer ID certificates and the notary key | the universal .tar.gz and the notarized .pkg |
    | linux    | nfpm and the OpenPGP key | .deb and .rpm for amd64 and arm64 |
    | sign     | minisign and its key | SHA256SUMS.minisig, checked against packaging/atrius-index.pub |
    | tag      | a clean tree | the release commit and the tag v<version>, pushed |
    | github   | gh and a token | a GitHub release on the private repository with every asset |
    | site     | the black-rainbow-labs-sites main checkout | blackrainbowlabs.com/downloads/atrius-index, deployed |
    | crates   | a crates.io token | atrius-index, then atrius-cli |
    | homebrew | the tap checkout or git | Formula/atrius-index.rb in Black-Rainbow-Labs/homebrew-atrius |

    Credentials are read from the sealed store Inillucent's release uses,
    %LOCALAPPDATA%\inillucent\signing and ...\apple, which hold Black Rainbow Labs' keys. The unsealed
    minisign key goes to the RAM disk and is deleted in a finally. A route whose need is missing is
    skipped with the reason; it is never reported as published.

.PARAMETER Part
    Which part of the version to raise: patch (the default), minor or major. Ignored with -Version.

.PARAMETER Version
    The exact version to release.

.PARAMETER Only
    Run only these routes.

.PARAMETER Skip
    Skip these routes.

.PARAMETER WhatIf
    Print the plan and change nothing.

.PARAMETER Current
    Release the version already in Cargo.toml instead of raising it.

.EXAMPLE
    pwsh packaging/ship.ps1 -WhatIf
    pwsh packaging/ship.ps1 -Part minor
    pwsh packaging/ship.ps1 -Current -Only site,homebrew
#>
[CmdletBinding()]
param(
    [ValidateSet('patch', 'minor', 'major')] [string] $Part = 'patch',
    [string] $Version,
    [string[]] $Only = @(),
    [string[]] $Skip = @(),
    [switch] $WhatIf,
    [switch] $Current,
    [string] $SitesRoot,
    [string] $TapPath
)

$ErrorActionPreference = 'Stop'
# `pwsh -File ship.ps1 -Only a,b` passes one string, so a comma list is split here.
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$Skip = @($Skip | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$script:Root = Split-Path -Parent $PSScriptRoot
$script:Dist = Join-Path $script:Root 'dist'
. (Join-Path $PSScriptRoot 'common.ps1')
. (Join-Path $PSScriptRoot 'macos/apple-credentials.ps1')
. (Join-Path $PSScriptRoot 'github-token.ps1')
$script:CrossBin = Get-CrossBin -Root $script:Root
$script:Zigbuild = Join-Path $script:CrossBin 'cargo-zigbuild.exe'
$script:Minisign = Join-Path $script:CrossBin 'minisign.exe'
if (-not $SitesRoot) { $SitesRoot = Join-Path (Split-Path -Parent $script:Root) 'black-rainbow-labs-sites' }
if (-not $TapPath) { $TapPath = Join-Path (Split-Path -Parent $script:Root) 'homebrew-atrius' }
$script:SitesRoot = $SitesRoot
$script:TapPath = $TapPath
$script:Downloads = Join-Path $SitesRoot 'sites/blackrainbowlabs/public/downloads/atrius-index'
$script:BaseUrl = 'https://blackrainbowlabs.com/downloads/atrius-index'
$script:Repo = 'Black-Rainbow-Labs/atrius-index'
$script:TapRepo = 'Black-Rainbow-Labs/homebrew-atrius'
$script:Scratch = @()

function Step-Version {
    <#
    .SYNOPSIS
        The next version after one, raising the given part.
    .PARAMETER From
        The current version.
    .PARAMETER Which
        patch, minor or major.
    #>
    param([string] $From, [string] $Which)
    $p = $From.Split('.') | ForEach-Object { [int] $_ }
    switch ($Which) {
        'major' { return "$($p[0] + 1).0.0" }
        'minor' { return "$($p[0]).$($p[1] + 1).0" }
        default { return "$($p[0]).$($p[1]).$($p[2] + 1)" }
    }
}

function Set-ReleaseVersion {
    <#
    .SYNOPSIS
        Writes the version into every file that carries it, and refreshes Cargo.lock.
    .PARAMETER New
        The version.
    #>
    param([string] $New)
    $manifest = Join-Path $script:Root 'Cargo.toml'
    $text = [System.IO.File]::ReadAllText($manifest)
    $text = [regex]::Replace($text, '(?ms)(\[workspace\.package\].*?version\s*=\s*")[^"]+(")', "`${1}$New`${2}", 1)
    $text = [regex]::Replace($text, '(atrius-index = \{ path = "crates/atrius-index", version = ")[^"]+(")', "`${1}$New`${2}")
    [System.IO.File]::WriteAllText($manifest, $text)
    & cargo update --manifest-path $manifest --workspace --offline 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { & cargo update --manifest-path $manifest --workspace | Out-Null }
}

function Import-Secrets {
    <#
    .SYNOPSIS
        Puts the signing keys and tokens into this process from the sealed store. Anything already in
        the environment wins.
    #>
    $store = Join-Path $env:LOCALAPPDATA 'inillucent\signing'
    $sealedMinisign = Join-Path $store 'minisign.key.sealed'
    if (-not $env:INILLUCENT_MINISIGN_KEY -and (Test-Path -LiteralPath $sealedMinisign)) {
        $keyFile = Join-Path (Get-AppleScratchDir) ('minisign-' + [guid]::NewGuid().ToString('N') + '.key')
        Set-Content -Path $keyFile -Value (Unprotect-AppleSecret -Path $sealedMinisign) -NoNewline
        $env:INILLUCENT_MINISIGN_KEY = $keyFile
        if ($null -eq $env:INILLUCENT_MINISIGN_PASSPHRASE) { $env:INILLUCENT_MINISIGN_PASSPHRASE = '' }
        $script:Scratch += $keyFile
    }
    $keyId = Join-Path $store 'gpg.keyid'
    if (-not $env:INILLUCENT_GPG_KEY -and (Test-Path -LiteralPath $keyId)) { $env:INILLUCENT_GPG_KEY = (Get-Content -LiteralPath $keyId -Raw).Trim() }
    $sealedGpg = Join-Path $store 'gpg.passphrase.sealed'
    if (-not $env:INILLUCENT_GPG_PASSPHRASE -and (Test-Path -LiteralPath $sealedGpg)) { $env:INILLUCENT_GPG_PASSPHRASE = Unprotect-AppleSecret -Path $sealedGpg }
    $sealedCrates = Join-Path $store 'crates.token.sealed'
    if (-not $env:CARGO_REGISTRY_TOKEN -and (Test-Path -LiteralPath $sealedCrates)) { $env:CARGO_REGISTRY_TOKEN = Unprotect-AppleSecret -Path $sealedCrates }
    $token = Resolve-GitHubToken
    if ($token) { $env:GH_TOKEN = $token }
}

function Remove-Secrets {
    <#
    .SYNOPSIS
        Deletes the unsealed files and clears the variables Import-Secrets set.
    #>
    foreach ($file in $script:Scratch) { Remove-Item -LiteralPath $file -Force -Confirm:$false -ErrorAction SilentlyContinue }
    foreach ($name in 'INILLUCENT_MINISIGN_KEY', 'INILLUCENT_GPG_PASSPHRASE', 'CARGO_REGISTRY_TOKEN', 'GH_TOKEN') {
        Remove-Item -Path "Env:\$name" -ErrorAction SilentlyContinue
    }
}

function Test-Url {
    <#
    .SYNOPSIS
        Fetches a URL until its body holds some text, for up to three minutes. Answers null when it
        does, or the reason it did not.
    .PARAMETER Url
        The URL.
    .PARAMETER Holds
        The text the body must hold.
    #>
    param([string] $Url, [string] $Holds)
    $deadline = (Get-Date).AddMinutes(3)
    $last = ''
    while ((Get-Date) -lt $deadline) {
        try {
            $body = (Invoke-WebRequest -Uri $Url -UseBasicParsing -Headers @{ 'Cache-Control' = 'no-cache' }).Content
            if ($body -is [byte[]]) { $body = [System.Text.Encoding]::UTF8.GetString($body) }
            if ($body.Contains($Holds)) { return $null }
            $last = "it does not hold $Holds"
        } catch { $last = $_.Exception.Message }
        Start-Sleep -Seconds 10
    }
    return "$Url : $last"
}

function Get-ReleaseAssets {
    <#
    .SYNOPSIS
        The files a release publishes: every name in SHA256SUMS, the sums, their signature.
    .PARAMETER Version
        The version.
    #>
    param([string] $Version)
    $sums = Join-Path $script:Dist 'SHA256SUMS'
    $names = (Get-Content -LiteralPath $sums) | ForEach-Object { ($_ -split '\s+')[-1] } | Where-Object { $_ }
    $files = @($names | ForEach-Object { Join-Path $script:Dist $_ })
    $files += $sums
    $signature = "$sums.minisig"
    if (Test-Path -LiteralPath $signature) { $files += $signature }
    return $files
}

function Get-Routes {
    <#
    .SYNOPSIS
        Every route, with what it needs, how it runs and how it is checked.
    .PARAMETER Version
        The version being released.
    #>
    param([string] $Version)


    return @(
        @{ Name = 'build'
            Needs = { if (-not (Test-Path -LiteralPath $script:Zigbuild)) { return "cargo-zigbuild is not in $($script:CrossBin)" }; $null }
            Run = { & (Join-Path $PSScriptRoot 'build.ps1') -Version $Version -Targets all }
            Verify = { foreach ($t in 'x86_64-pc-windows-msvc.zip', 'x86_64-unknown-linux-gnu.tar.gz', 'aarch64-unknown-linux-gnu.tar.gz') { if (-not (Test-Path -LiteralPath (Join-Path $script:Dist "atrius-index-$Version-$t"))) { return "atrius-index-$Version-$t was not written" } }; $null } },
        @{ Name = 'macos'
            Needs = { $c = Test-AppleCredentials; if (-not ($c.Application -and $c.Installer -and $c.NotaryKey)) { return "the Apple credentials are not all in $($c.Directory)" }; $null }
            Run = { & (Join-Path $PSScriptRoot 'macos/release-macos.ps1') -Version $Version }
            Verify = { if (-not (Test-Path -LiteralPath (Join-Path $script:Dist "atrius-index-$Version.pkg"))) { return 'the .pkg was not written' }; $null } },
        @{ Name = 'linux'
            Needs = { if (-not $env:INILLUCENT_GPG_KEY) { return 'no OpenPGP key id' }; $null }
            Run = { & (Join-Path $PSScriptRoot 'linux/package-linux.ps1') -Version $Version }
            Verify = { if (-not (Test-Path -LiteralPath (Join-Path $script:Dist "atrius-index_${Version}_amd64.deb"))) { return 'the .deb was not written' }; $null } },
        @{ Name = 'sign'
            Needs = { if (-not (Test-Path -LiteralPath $script:Minisign)) { return 'minisign is missing' }; if (-not $env:INILLUCENT_MINISIGN_KEY) { return 'no minisign key' }; $null }
            Run = {
                $sums = Update-Sha256Sums -Dist $script:Dist -Version $Version
                Remove-Item -LiteralPath "$sums.minisig" -Force -Confirm:$false -ErrorAction SilentlyContinue
                $comment = "atrius-index $Version, signed on $(Get-Date -Format 'yyyy-MM-dd')"
                $env:INILLUCENT_MINISIGN_PASSPHRASE | & $script:Minisign -S -s $env:INILLUCENT_MINISIGN_KEY -m $sums -c $comment -t $comment
                if ($LASTEXITCODE -ne 0) { throw "minisign failed with $LASTEXITCODE" }
            }
            Verify = { & $script:Minisign -V -p (Join-Path $PSScriptRoot 'atrius-index.pub') -m (Join-Path $script:Dist 'SHA256SUMS') | Out-Null; if ($LASTEXITCODE -ne 0) { return 'the signature does not verify' }; $null } },
        @{ Name = 'tag'
            Needs = { $null }
            Run = {
                & git -C $script:Root add Cargo.toml Cargo.lock
                & git -C $script:Root commit -q -m "Atrius $Version"
                & git -C $script:Root tag -a "v$Version" -m "Atrius $Version"
                & git -C $script:Root push -q origin HEAD:main
                & git -C $script:Root push -q origin "v$Version"
                if ($LASTEXITCODE -ne 0) { throw 'git push failed' }
            }
            Verify = { if (-not ((& git -C $script:Root ls-remote --tags origin "v$Version") -match "v$Version")) { return "origin has no tag v$Version" }; $null } },
        @{ Name = 'github'
            Needs = { if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { return 'gh is not installed' }; & gh api user --jq .login *> $null; if ($LASTEXITCODE -ne 0) { return 'gh has no working token' }; $null }
            Run = {
                $assets = Get-ReleaseAssets -Version $Version
                & gh release view "v$Version" --repo $script:Repo *> $null
                if ($LASTEXITCODE -eq 0) {
                    & gh release upload "v$Version" @assets --repo $script:Repo --clobber
                } else {
                    & gh release create "v$Version" @assets --repo $script:Repo --title "Atrius $Version" --notes "Downloads, install scripts and checksums: $($script:BaseUrl)/"
                }
                if ($LASTEXITCODE -ne 0) { throw 'gh release failed' }
                & gh release edit "v$Version" --repo $script:Repo --draft=false | Out-Null
            }
            Verify = { $draft = & gh release view "v$Version" --repo $script:Repo --json isDraft --jq .isDraft; if ($draft -ne 'false') { return 'the release is missing or a draft' }; $null } },
        @{ Name = 'site'
            Needs = { if (-not (Test-Path -LiteralPath (Join-Path $script:SitesRoot 'tools/deploy.mjs'))) { return "no sites checkout at $($script:SitesRoot)" }; $sh = [System.IO.File]::ReadAllText((Join-Path $PSScriptRoot 'install.sh')); if ($sh.Contains("`r")) { return 'install.sh has a carriage return' }; $null }
            Run = {
                New-Item -ItemType Directory -Force -Path $script:Downloads | Out-Null
                foreach ($file in (Get-ReleaseAssets -Version $Version)) { Copy-Item -LiteralPath $file -Destination $script:Downloads -Force }
                foreach ($file in 'install.sh', 'install.ps1', 'atrius-index.pub') { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $file) -Destination $script:Downloads -Force }
                [System.IO.File]::WriteAllText((Join-Path $script:Downloads 'VERSION'), "$Version`n")
                & node (Join-Path $script:SitesRoot 'tools/deploy.mjs') blackrainbowlabs
                if ($LASTEXITCODE -ne 0) { throw "the site deploy failed with $LASTEXITCODE" }
            }
            Verify = {
                $why = Test-Url -Url "$($script:BaseUrl)/VERSION" -Holds $Version
                if ($why) { return $why }
                foreach ($file in (Get-ReleaseAssets -Version $Version)) {
                    $name = Split-Path -Leaf $file
                    # The bytes fetched, rather than a Content-Length: the edge answers a text file
                    # compressed, with no length header at all.
                    $size = [int64](& curl.exe -sL -o NUL -w '%{size_download}' "$($script:BaseUrl)/$name")
                    if ($size -ne (Get-Item -LiteralPath $file).Length) { return "$name is served at $size bytes and is $((Get-Item -LiteralPath $file).Length)" }
                }
                $null
            } },
        @{ Name = 'crates'
            Needs = { if (-not $env:CARGO_REGISTRY_TOKEN) { return 'no crates.io token' }; $null }
            Run = {
                # cargo publish builds the crates to check them, and onig_sys needs the MSVC environment.
                Import-MsvcEnvironment
                foreach ($crate in 'atrius-index', 'atrius-cli') {
                    $published = $false
                    try { $published = ((Invoke-WebRequest -Uri "https://crates.io/api/v1/crates/$crate/$Version" -UseBasicParsing -UserAgent 'atrius-ship').StatusCode -eq 200) } catch {}
                    if ($published) { continue }
                    & cargo publish --manifest-path (Join-Path $script:Root "crates/$crate/Cargo.toml") --locked
                    if ($LASTEXITCODE -ne 0) { throw "cargo publish $crate failed with $LASTEXITCODE" }
                    Start-Sleep -Seconds 20
                }
            }
            Verify = { foreach ($crate in 'atrius-index', 'atrius-cli') { $why = Test-Url -Url "https://crates.io/api/v1/crates/$crate" -Holds "`"$Version`""; if ($why) { return $why } }; $null } },
        @{ Name = 'homebrew'
            Needs = { $null }
            Run = {
                if (-not (Test-Path -LiteralPath (Join-Path $script:TapPath '.git'))) {
                    & git clone -q "https://github.com/$($script:TapRepo).git" $script:TapPath
                    if ($LASTEXITCODE -ne 0) { throw "cannot clone $($script:TapRepo)" }
                }
                & git -C $script:TapPath pull -q --ff-only 2>$null
                $sums = @{}
                foreach ($line in (Get-Content -LiteralPath (Join-Path $script:Dist 'SHA256SUMS'))) { $f = $line -split '\s+'; if ($f.Count -ge 2) { $sums[$f[-1]] = $f[0] } }
                $formula = [System.IO.File]::ReadAllText((Join-Path $PSScriptRoot 'homebrew/atrius-index.rb')).Replace('@VERSION@', $Version)
                foreach ($triple in 'universal-apple-darwin', 'x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu') {
                    $digest = $sums["atrius-index-$Version-$triple.tar.gz"]
                    if (-not $digest) { throw "SHA256SUMS has no atrius-index-$Version-$triple.tar.gz" }
                    $formula = $formula.Replace("@SHA256_$($triple.ToUpper().Replace('-', '_'))@", $digest)
                }
                New-Item -ItemType Directory -Force -Path (Join-Path $script:TapPath 'Formula') | Out-Null
                [System.IO.File]::WriteAllText((Join-Path $script:TapPath 'Formula/atrius-index.rb'), $formula.Replace("`r`n", "`n"))
                & git -C $script:TapPath add Formula/atrius-index.rb
                & git -C $script:TapPath commit -q -m "atrius-index $Version"
                & git -C $script:TapPath push -q origin HEAD:main
                if ($LASTEXITCODE -ne 0) { throw 'the tap push failed' }
            }
            Verify = { Test-Url -Url "https://raw.githubusercontent.com/$($script:TapRepo)/main/Formula/atrius-index.rb" -Holds "version `"$Version`"" } }
    )
}

# ---------------------------------------------------------------------------
$checkoutVersion = Get-WorkspaceVersion -Root $script:Root
if (-not $Version) { $Version = if ($Current) { $checkoutVersion } else { Step-Version -From $checkoutVersion -Which $Part } }
if ([version]$Version -lt [version]$checkoutVersion) { throw "$Version is older than the checkout's $checkoutVersion" }
$dirty = & git -C $script:Root status --porcelain
if ($dirty -and -not $WhatIf) { throw "the working tree is not clean:`n$($dirty -join "`n")" }

Write-Host "Atrius $Version (the checkout says $checkoutVersion)"
Import-Secrets
$outcomes = [ordered]@{}
try {
    $routes = Get-Routes -Version $Version
    $plan = @()
    Write-Host '== preflight'
    foreach ($route in $routes) {
        $wanted = ($Only.Count -eq 0 -or $Only -contains $route.Name) -and ($Skip -notcontains $route.Name)
        $why = if ($wanted) { & $route.Needs } else { 'not asked for' }
        $plan += @{ Route = $route; Blocked = $why }
        Write-Host ("  [{0}] {1}{2}" -f ($(if ($why) { 'skip' } else { 'run ' })), $route.Name, $(if ($why) { ": $why" } else { '' }))
    }
    if ($WhatIf) { return }

    if ($Version -ne $checkoutVersion) {
        Write-Host '== version'
        Set-ReleaseVersion -New $Version
    }
    foreach ($entry in $plan) {
        $name = $entry.Route.Name
        if ($entry.Blocked) { $outcomes[$name] = "skipped: $($entry.Blocked)"; continue }
        Write-Host "== $name"
        try {
            & $entry.Route.Run
            $why = & $entry.Route.Verify
            $outcomes[$name] = if ($why) { "FAILED: $why" } else { 'published' }
        } catch {
            $outcomes[$name] = "FAILED: $($_.Exception.Message)"
        }
    }
} finally {
    Remove-Secrets
}

Write-Host ''
Write-Host "== Atrius $Version"
$failed = $false
foreach ($key in $outcomes.Keys) {
    $line = "  {0,-9} {1}" -f $key, $outcomes[$key]
    if ($outcomes[$key] -like 'FAILED*') { $failed = $true; Write-Host $line -ForegroundColor Red } else { Write-Host $line }
}
if ($failed) {
    Write-Host 're-run the failed routes with -Current -Only <names>'
    exit 1
}
