#!/usr/bin/env sh
# Skopos installer.
#
#   curl -fsSL https://raw.githubusercontent.com/Sadboy-s-Development-Group/skopos/master/install.sh | sh
#
# Downloads the latest GitHub Release tarball for the current platform,
# verifies the SHA-256 checksum, and installs the `skopos` binary into
# $INSTALL_DIR (defaults to ~/.local/bin).
#
# Environment knobs:
#   INSTALL_DIR   target directory for the binary (default ~/.local/bin)
#   SKOPOS_VERSION  override the release tag to install (default: latest)

set -eu

REPO="Sadboy-s-Development-Group/skopos"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

err() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }
log() { printf '  %s\n' "$*"; }

need() {
  command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"
}

need curl
need tar
need uname

# Pick a sha256 verifier if one is available; verification is optional
# but strongly recommended.
sha_cmd=""
if command -v sha256sum >/dev/null 2>&1; then
  sha_cmd="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
  sha_cmd="shasum -a 256"
fi

# Map uname output to the Rust target triple used by the release assets.
os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Linux-x86_64)   target="x86_64-unknown-linux-gnu" ;;
  Linux-aarch64)  err "no aarch64-linux binary yet — build from source: cargo install skopos-cli --locked" ;;
  Darwin-x86_64)  err "no x86_64-darwin binary yet — build from source: cargo install skopos-cli --locked" ;;
  Darwin-arm64)   err "no aarch64-darwin binary yet — build from source: cargo install skopos-cli --locked" ;;
  *)              err "unsupported platform: $os $arch — build from source: cargo install skopos-cli --locked" ;;
esac

# Resolve the release tag we want to download. We hit `/releases` rather
# than `/releases/latest` so pre-release tags (e.g. `v0.2.0-beta.1`) are
# considered — GitHub excludes pre-releases from the `/latest` endpoint.
if [ "${SKOPOS_VERSION:-}" = "" ]; then
  log "Resolving latest release…"
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=1" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
    | head -n1)"
  [ -n "$tag" ] || err "could not resolve latest release tag"
else
  tag="$SKOPOS_VERSION"
  case "$tag" in v*) ;; *) tag="v$tag" ;; esac
fi
version="${tag#v}"

asset="skopos-${version}-${target}.tar.gz"
checksum="${asset}.sha256"
base="https://github.com/$REPO/releases/download/${tag}"

log "Downloading ${asset}…"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
curl -fsSL -o "$tmpdir/$asset"    "$base/$asset"
curl -fsSL -o "$tmpdir/$checksum" "$base/$checksum"

if [ -n "$sha_cmd" ]; then
  log "Verifying checksum…"
  # The release workflow writes the .sha256 file with a `dist/<asset>`
  # path embedded, so `sha256sum -c` fails when we run it from /tmp.
  # Compare hashes directly instead.
  expected="$(awk '{print $1}' "$tmpdir/$checksum")"
  actual="$(cd "$tmpdir" && $sha_cmd "$asset" | awk '{print $1}')"
  [ "$expected" = "$actual" ] \
    || err "checksum verification failed for $asset (expected $expected, got $actual)"
else
  log "Skipping checksum verification — install sha256sum or shasum to enable it."
fi

log "Extracting…"
tar -xzf "$tmpdir/$asset" -C "$tmpdir"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmpdir/skopos-${version}-${target}/skopos" "$INSTALL_DIR/skopos"

log "Installed skopos $version to $INSTALL_DIR/skopos"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '\n  Note: %s is not on your $PATH. Add it with:\n\n    export PATH="%s:$PATH"\n\n' \
      "$INSTALL_DIR" "$INSTALL_DIR" ;;
esac
