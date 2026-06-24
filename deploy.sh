#!/bin/bash
set -euo pipefail

cargo install --force --path . --root ~/.local
echo "Installed regex-replace-mcp to ~/.local/bin/"
