#!/bin/sh
# Download the latest wrtforge release for this machine, verify its checksum, and run it. Nothing is installed.
#   curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/get.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/mralinp/wrtforge/main/get.sh | sh -s -- 192.168.1.1 --dry-run
set -eu
repo=mralinp/wrtforge
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) t=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) t=aarch64-unknown-linux-musl ;;
  Darwin-x86_64) t=x86_64-apple-darwin ;;
  Darwin-arm64) t=aarch64-apple-darwin ;;
  *) echo "wrtforge: unsupported system $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url=https://github.com/$repo/releases/latest/download/wrtforge-$t.tar.gz
curl -fsSL "$url" -o "$tmp/w.tgz"
curl -fsSL "$url.sha256" -o "$tmp/w.sha"
want=$(cut -d' ' -f1 "$tmp/w.sha")
if command -v sha256sum >/dev/null 2>&1; then got=$(sha256sum "$tmp/w.tgz" | cut -d' ' -f1); else got=$(shasum -a 256 "$tmp/w.tgz" | cut -d' ' -f1); fi
[ "$want" = "$got" ] || { echo "wrtforge: checksum mismatch, refusing to run" >&2; exit 1; }
tar -xzf "$tmp/w.tgz" -C "$tmp"
set +e
if [ -t 0 ]; then "$tmp/wrtforge" "$@"; else "$tmp/wrtforge" "$@" </dev/tty; fi
exit $?
