#!/usr/bin/env bash
#
# Installs Atrius, the code index, for the current user on macOS or Linux.
#
#   curl -fsSL https://blackrainbowlabs.com/downloads/atrius-index/install.sh | sh
#
# It downloads the release archive for this machine, checks its SHA-256 against
# the release's SHA256SUMS, unpacks it into ~/.local/share/atrius-index and
# links atrius and atrius-mcp into ~/.local/bin. Nothing is written outside the
# home directory and nothing needs sudo.
#
#   --version X.Y.Z   install a specific release rather than the latest
#   --base-url URL    download from somewhere else
#   --from-dist       install the archive in dist/ instead of downloading
#   --prefix DIR      install somewhere else (default ~/.local/share/atrius-index)
#   --bin-dir DIR     link the programs somewhere else (default ~/.local/bin)
#   --uninstall       remove what this installed
#
# On macOS this needs no Apple account: a file fetched with curl carries no
# quarantine attribute. The signed and notarized .pkg beside the archives is
# for somebody who would rather double click.

set -eu
# `pipefail` is not POSIX, and on Debian and Ubuntu `sh` is dash, which stops
# at `set -o pipefail`. It is enabled only where the shell has it.
# shellcheck disable=SC3040
(set -o pipefail 2>/dev/null) && set -o pipefail || true

base_url="https://blackrainbowlabs.com/downloads/atrius-index"
version=""
from_dist=0
prefix="${HOME}/.local/share/atrius-index"
bin_dir="${HOME}/.local/bin"
uninstall=0

while [ $# -gt 0 ]; do
  case "$1" in
    --version) version="$2"; shift 2 ;;
    --base-url) base_url="${2%/}"; shift 2 ;;
    --from-dist) from_dist=1; shift ;;
    --prefix) prefix="$2"; shift 2 ;;
    --bin-dir) bin_dir="$2"; shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

programs="atrius atrius-mcp"

if [ "$uninstall" -eq 1 ]; then
  if [ -x "$prefix/bin/atrius" ]; then "$prefix/bin/atrius" service uninstall >/dev/null 2>&1 || true; fi
  for program in $programs; do
    # Only a link this script made. `-L` tells a link from a program somebody put there.
    if [ -L "$bin_dir/$program" ]; then rm -f "$bin_dir/$program"; fi
  done
  if [ -d "$prefix" ]; then rm -rf "$prefix"; echo "removed $prefix"; fi
  echo "uninstalled. The indexes stay in your cache folder; atrius-index under it can be deleted."
  exit 0
fi

kernel="$(uname -s)"
machine="$(uname -m)"
case "$kernel/$machine" in
  Darwin/arm64)  target="universal-apple-darwin" ;;
  Darwin/x86_64) target="universal-apple-darwin" ;;
  Linux/x86_64)  target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64) target="aarch64-unknown-linux-gnu" ;;
  Linux/arm64)   target="aarch64-unknown-linux-gnu" ;;
  *)
    echo "there is no Atrius release for $kernel/$machine yet." >&2
    echo "Build from source instead:  cargo install atrius-cli" >&2
    exit 1 ;;
esac

# Verifies a downloaded archive against the release's SHA256SUMS.
verify() {
  archive="$1"
  sums="$2"
  name="$(basename "$archive")"
  # `\r` is interpreted by tr. A literal carriage return between the quotes does
  # not survive line ending normalisation.
  expected="$(tr -d '\r' < "$sums" | awk -v want="$name" '$NF == want { print $1 }' | head -1)"
  if [ -z "$expected" ]; then
    echo "SHA256SUMS does not list $name" >&2
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive" | awk '{print $1}')"
  else
    actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  fi
  if [ "$actual" != "$expected" ]; then
    echo "$name does not match its published checksum." >&2
    echo "  expected $expected" >&2
    echo "  got      $actual" >&2
    exit 1
  fi
  echo "checksum ok ($expected)"
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [ "$from_dist" -eq 1 ]; then
  root="$(cd "$(dirname "$0")/.." && pwd)"
  dist="$root/dist"
  if [ -z "$version" ]; then
    found="$(ls "$dist"/atrius-index-*-"$target".tar.gz 2>/dev/null | head -1 || true)"
    [ -n "$found" ] || { echo "no archive for $target in $dist." >&2; exit 1; }
    version="$(basename "$found" | sed "s/^atrius-index-//; s/-$target\.tar\.gz$//")"
  fi
  archive="$dist/atrius-index-$version-$target.tar.gz"
  [ -f "$archive" ] || { echo "$archive does not exist." >&2; exit 1; }
  verify "$archive" "$dist/SHA256SUMS"
else
  if [ -z "$version" ]; then
    version="$(curl -fsSL "$base_url/VERSION" | tr -d '\r\n ')"
    [ -n "$version" ] || { echo "could not read $base_url/VERSION; pass --version" >&2; exit 1; }
  fi
  name="atrius-index-$version-$target.tar.gz"
  archive="$work/$name"
  curl -fsSL "$base_url/SHA256SUMS" -o "$work/SHA256SUMS.raw"
  tr -d '\r' < "$work/SHA256SUMS.raw" > "$work/SHA256SUMS"
  if ! awk -v want="$name" '$NF == want { found = 1 } END { exit !found }' "$work/SHA256SUMS"; then
    echo "Atrius $version has no build for $target." >&2
    echo "  published in this release:" >&2
    awk '{ print "    " $NF }' "$work/SHA256SUMS" >&2
    echo "  Build it from source instead:  cargo install atrius-cli" >&2
    exit 1
  fi
  echo "downloading Atrius $version for $target..."
  curl -fsSL "$base_url/$name" -o "$archive"
  verify "$archive" "$work/SHA256SUMS"
fi

mkdir -p "$work/unpacked"
tar --directory "$work/unpacked" -xzf "$archive"
inner="$(find "$work/unpacked" -mindepth 1 -maxdepth 1 -type d | head -1)"
[ -n "$inner" ] || { echo "the archive holds no directory" >&2; exit 1; }

rm -rf "$prefix"
mkdir -p "$prefix"
cp -R "$inner"/. "$prefix"/

mkdir -p "$bin_dir"
for program in $programs; do
  chmod +x "$prefix/bin/$program"
  ln -sf "$prefix/bin/$program" "$bin_dir/$program"
done

echo
echo "Atrius $version is installed in $prefix"
"$prefix/bin/atrius" --version

case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *)
    case "$(basename "${SHELL:-sh}")" in
      zsh) profile='~/.zshrc'; reload='exec zsh' ;;
      bash) profile='~/.bashrc'; reload='exec bash' ;;
      fish) profile='~/.config/fish/config.fish'; reload='exec fish' ;;
      *) profile='~/.profile'; reload='. ~/.profile' ;;
    esac
    echo
    echo "$bin_dir is not on your PATH. Add it:"
    if [ "$profile" = '~/.config/fish/config.fish' ]; then
      echo "  echo 'fish_add_path $bin_dir' >> $profile && $reload"
    else
      echo "  echo 'export PATH=\"$bin_dir:\$PATH\"' >> $profile && $reload"
    fi
    ;;
esac

cat <<INSTRUCTIONS

Try, inside any git checkout:
  atrius find TODO
  atrius def main
  atrius doctor

Give an agent the same queries:
  claude mcp add atrius -- $bin_dir/atrius-mcp

Keep a checkout indexed at all times:
  atrius watch add ~/code/my-project && atrius service install
INSTRUCTIONS
