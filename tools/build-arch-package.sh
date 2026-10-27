#!/usr/bin/env bash
set -euo pipefail
source_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
output="${1:-$source_dir/dist}"
if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
  git -C "$source_dir" diff --exit-code HEAD -- >/dev/null
fi
source_commit="$(git -C "$source_dir" rev-parse HEAD)"
mkdir -p -- "$output"
output="$(realpath -- "$output")"
build_dir="$(mktemp -d -t qoda-makepkg-XXXXXXXX)"
trap 'rm -rf -- "$build_dir"' EXIT
cp -- "$source_dir/packaging/arch/PKGBUILD" "$build_dir/PKGBUILD"
export QODA_SOURCE="$source_dir" PKGDEST="$output" PACKAGER='Qoda Player CI <build@qoda.invalid>'
# Explicit target location is shared with package() and avoids makepkg's srcdir.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$source_dir/target}"
cd -- "$build_dir"
makepkg --noconfirm --nodeps
[[ "$(git -C "$source_dir" rev-parse HEAD)" == "$source_commit" ]] || { echo 'Source revision changed during package build' >&2; exit 1; }
cd -- "$output"
sha256sum -- qoda-player-*.pkg.tar.zst > SHA256SUMS
printf '%s\n' "$source_commit" > COMMIT
