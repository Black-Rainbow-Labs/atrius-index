#!/usr/bin/env bash
#
# Checks a published Atrius release by installing it, the way a stranger would.
#
#   bash packaging/verify-installs.sh 0.1.0
#
# Everything is fetched from blackrainbowlabs.com and GitHub, never from dist/: the question is
# whether somebody else can install the release. A check that cannot run on this machine says SKIP
# with the reason.

set -u
version="${1:?usage: verify-installs.sh <version>}"
base="https://blackrainbowlabs.com/downloads/atrius-index"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

ok()   { echo "ok    $*"; }
fail() { echo "FAIL  $*"; failures=$((failures + 1)); }
skip() { echo "SKIP  $*"; }

# 1. VERSION names this release.
served="$(curl -fsSL "$base/VERSION" | tr -d '\r\n ')"
[ "$served" = "$version" ] && ok "VERSION is $version" || fail "VERSION is '$served'"

# 2. Every name in SHA256SUMS is served, and hashes to what the file says.
curl -fsSL "$base/SHA256SUMS" -o "$work/SHA256SUMS" || fail "SHA256SUMS is not served"
while read -r digest name; do
  [ -n "$name" ] || continue
  if curl -fsSL "$base/$name" -o "$work/$name"; then
    actual="$(sha256sum "$work/$name" | awk '{print $1}')"
    [ "$actual" = "$digest" ] && ok "$name" || fail "$name hashes to $actual"
  else
    fail "$name is listed and not served"
  fi
done < <(tr -d '\r' < "$work/SHA256SUMS")

# 3. The signature over SHA256SUMS verifies with the published key.
curl -fsSL "$base/SHA256SUMS.minisig" -o "$work/SHA256SUMS.minisig" || fail "SHA256SUMS.minisig is not served"
minisign_bin="$(command -v minisign || true)"
[ -z "$minisign_bin" ] && [ -x "$here/../tools/cross/bin/minisign.exe" ] && minisign_bin="$here/../tools/cross/bin/minisign.exe"
if [ -n "$minisign_bin" ]; then
  "$minisign_bin" -Vm "$work/SHA256SUMS" -x "$work/SHA256SUMS.minisig" -p "$here/atrius-index.pub" >/dev/null \
    && ok "SHA256SUMS signature" || fail "SHA256SUMS signature does not verify"
else
  skip "no minisign on this machine"
fi

# 4. install.sh on Linux, in WSL, into a scratch home.
if command -v wsl.exe >/dev/null 2>&1; then
  out="$(wsl.exe -e bash -lc "export HOME=\$(mktemp -d); curl -fsSL $base/install.sh | sh -s -- --version $version >/dev/null && \$HOME/.local/bin/atrius --version && mkdir -p \$HOME/p/src && printf 'fn retry_write() {}\n' > \$HOME/p/src/lib.rs && git init -q \$HOME/p && ATRIUS_INDEX_CACHE=\$HOME/c \$HOME/.local/bin/atrius find retry_write --root \$HOME/p && ATRIUS_INDEX_CACHE=\$HOME/c \$HOME/.local/bin/atrius stop --root \$HOME/p" 2>&1)"
  echo "$out" | grep -q "atrius $version" && echo "$out" | grep -q "src/lib.rs" && ok "install.sh in WSL, and a search" || fail "install.sh in WSL: $out"
  out="$(wsl.exe -e bash -lc "d=\$(mktemp -d) && curl -fsSL -o \$d/a.deb $base/atrius-index_${version}_amd64.deb && dpkg-deb -x \$d/a.deb \$d/root && \$d/root/usr/bin/atrius --version" 2>&1)"
  echo "$out" | grep -q "atrius $version" && ok ".deb unpacks and runs in WSL" || fail ".deb in WSL: $out"
else
  skip "no WSL for install.sh and the .deb"
fi

# 5. install.ps1 on Windows, into a scratch folder with no PATH change.
if command -v pwsh >/dev/null 2>&1 && [ "${OS:-}" = "Windows_NT" ]; then
  prefix="$(cygpath -w "$work")\\win"
  out="$(pwsh -NoProfile -Command "& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing '$base/install.ps1').Content)) -Version $version -Prefix '$prefix' -NoPath | Out-Null; & '$prefix\\bin\\atrius.exe' --version" 2>&1)"
  echo "$out" | grep -q "atrius $version" && ok "install.ps1" || fail "install.ps1: $out"
else
  skip "not Windows, so install.ps1 is not run"
fi

# 6. The Homebrew formula names this version and the published checksums.
formula="$(curl -fsSL https://raw.githubusercontent.com/Black-Rainbow-Labs/homebrew-atrius/main/Formula/atrius-index.rb)"
echo "$formula" | grep -q "version \"$version\"" && ok "formula version" || fail "the formula does not name $version"
for triple in universal-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  digest="$(tr -d '\r' < "$work/SHA256SUMS" | awk -v want="atrius-index-$version-$triple.tar.gz" '$NF == want { print $1 }')"
  [ -n "$digest" ] && echo "$formula" | grep -q "$digest" && ok "formula checksum for $triple" || fail "formula checksum for $triple"
done

# 7. crates.io lists the version.
for crate in atrius-index atrius-cli; do
  curl -fsSL -A atrius-verify "https://crates.io/api/v1/crates/$crate" | grep -q "\"$version\"" && ok "crates.io $crate" || fail "crates.io $crate"
done

echo
[ "$failures" -eq 0 ] && echo "every check passed" || { echo "$failures check(s) failed"; exit 1; }
