#!/usr/bin/env bash
# Format, lint, test and check that generated docs are current.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
diff <(cargo run -q -p lazydisk-core --example keymap) docs/KEYMAP.md >/dev/null \
  || { echo "docs/KEYMAP.md is stale: cargo run -p lazydisk-core --example keymap > docs/KEYMAP.md"; exit 1; }
echo "all checks passed"
