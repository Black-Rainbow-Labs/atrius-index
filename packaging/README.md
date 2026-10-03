# Packaging

Everything here is driven by one command, run on the Windows release machine:

```powershell
pwsh packaging/ship.ps1 -WhatIf     # the plan: which routes run, and why any will not
pwsh packaging/ship.ps1             # a patch release
pwsh packaging/ship.ps1 -Part minor
pwsh packaging/ship.ps1 -Current -Only site,homebrew   # publish the version already built again
```

Do not run the scripts below by hand for a release. `ship.ps1` runs them in the right order and then
asks every destination what it serves.

| File | What it does |
|---|---|
| `ship.ps1` | The release. Its header lists every route |
| `build.ps1` | Builds the five targets at once and writes the Windows and Linux archives |
| `macos/release-macos.ps1` | Universal binaries, Developer ID signatures, the macOS archives, the `.pkg`, notarization |
| `linux/package-linux.ps1` | `.deb` and `.rpm` with nfpm, signed with OpenPGP |
| `common.ps1` | The version, the cross toolchain, the MSVC environment, the archive layout, `SHA256SUMS` |
| `install.sh`, `install.ps1` | The installers published beside the downloads |
| `homebrew/atrius-index.rb` | The formula template; `ship.ps1` fills it and pushes it to the tap |
| `atrius-index.pub` | The minisign public key `SHA256SUMS.minisig` is checked with |

## Where a release goes

| Destination | URL |
|---|---|
| Downloads, install scripts, checksums | https://blackrainbowlabs.com/downloads/atrius-index/ |
| GitHub release, private | https://github.com/Black-Rainbow-Labs/atrius-index/releases |
| crates.io | https://crates.io/crates/atrius-index and https://crates.io/crates/atrius-cli |
| Homebrew tap | https://github.com/Black-Rainbow-Labs/homebrew-atrius |

The source repository is private, so its GitHub release assets cannot be downloaded without a login.
The installers and the Homebrew formula therefore download from blackrainbowlabs.com, which serves
`/downloads/` through the Cloudflare cache from the `black-rainbow-labs-sites` repository.

## Credentials

Black Rainbow Labs signs Atrius and Inillucent with the same keys, so the release reads the sealed
store Inillucent's release created. Each file is sealed with DPAPI under the release account and is
worthless on another machine.

| File under `%LOCALAPPDATA%\inillucent\` | Used for |
|---|---|
| `apple\developer-id-application.key.sealed` and `.cer` | signing the programs |
| `apple\developer-id-installer.key.sealed` and `.cer` | signing the `.pkg` |
| `apple\notary-key.json.sealed` | Apple's notary service |
| `signing\minisign.key.sealed` | `SHA256SUMS.minisig` |
| `signing\gpg.keyid`, `signing\gpg.passphrase.sealed` | the `.deb` and `.rpm` signatures |
| `signing\crates.token.sealed` | crates.io |

GitHub is reached with the token Git Credential Manager already holds, read with
`git credential fill`.

## The cross toolchain

zig, cargo-zigbuild, rcodesign, nfpm and minisign, at pinned versions, in the gitignored
`tools/cross/bin`. `pwsh tools/cross/fetch-toolchain.ps1` downloads them. `common.ps1` also finds the
copy in a sibling Inillucent checkout, since both products are released from the same machine.

## Checking a release by installing it

```sh
bash packaging/verify-installs.sh <version>
```

It fetches from blackrainbowlabs.com, never from `dist/`: it checks every name in the published
`SHA256SUMS` and its signature, runs `install.sh` and the `.deb` in WSL, runs `install.ps1` into a
scratch folder, and compares the Homebrew formula's checksums with the published ones.
