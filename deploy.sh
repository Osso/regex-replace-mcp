#!/bin/bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$repo_dir"

cargo build --release --bins
install -Dm755 target/release/regex-replace-mcp "$HOME/.local/bin/regex-replace-mcp"
install -Dm755 target/release/regex-replace-json "$HOME/.local/bin/regex-replace-json"
pi install "$repo_dir"

echo "Installed regex-replace-mcp, regex-replace-json, and the native Pi extension."
