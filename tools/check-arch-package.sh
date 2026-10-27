#!/usr/bin/env bash
set -euo pipefail
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
archive="$(realpath -- "${1:?Pass the package archive}")"
if (( EUID == 0 )); then
  exec python3 "$source_dir/tools/check-arch-package.py" "$archive"
fi
# Host filesystem read-only; pacman can write only within this private directory.
scratch="$(mktemp -d -t qoda-pacman-sandbox-XXXXXXXX)"
trap 'rm -rf -- "$scratch"' EXIT
bwrap --unshare-user --uid 0 --gid 0 --unshare-pid --unshare-net --die-with-parent \
  --ro-bind / / --proc /proc --dev /dev --bind "$scratch" "$scratch" \
  --setenv TMPDIR "$scratch" python3 "$source_dir/tools/check-arch-package.py" "$archive"
