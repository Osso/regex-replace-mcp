#!/bin/bash
set -euo pipefail

cargo fmt --check
cargo test
npm test
npm run typecheck
