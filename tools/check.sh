#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

cargo fmt --all -- --check
CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked

QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software /usr/lib/qt6/bin/qmltestrunner -input tests/qml
