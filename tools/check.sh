#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

cargo fmt --all -- --check
CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo test -p qoda-backend --example continuous_pcm --locked
cargo build -p qoda-backend --example continuous_pcm --locked
target_dir="$(cargo metadata --no-deps --format-version=1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
python3 tools/check-continuous-pcm.py --example "$target_dir/debug/examples/continuous_pcm"

QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software /usr/lib/qt6/bin/qmltestrunner -input tests/qml

python3 -m unittest discover -s tests/python
