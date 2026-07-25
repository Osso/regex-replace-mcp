# Pi Regex Replace Tool

The Pi regex replacement tool exposes the repository's Rust replacement engine through a native Pi extension. The shared engine also remains available to the MCP server and JSON CLI.

## What it must do

### Shared replacement behavior

- [x] Preserve existing Rust-regex capture-group, literal-dollar, and escape-sequence behavior.
- [x] Plan all matched-file changes before writing.
- [x] Require an exact expected match count for Pi/JSON requests and reject mismatches without modifying files.
- [x] Reject invalid regexes, binary/non-UTF-8 inputs, zero matches, and configured file, byte, or match limit violations without modifying files.
- [x] Respect `.gitignore` while expanding file globs.
- [x] Return a stable plan hash and reject application when the files no longer match the approved plan.
- [x] Produce unified diffs for every changed file in dry-run and applied results.
- [x] Replace each file with an atomic rename and roll back already-written files if a later rename fails. This is transaction-like recovery, not literal cross-file atomic visibility.

### Interfaces

- [x] Preserve existing MCP search and replacement behavior.
- [x] Provide a two-phase JSON request-file CLI for planning and applying frozen replacement targets.
- [ ] Register a native Pi `regex_replace` tool with strict parameters and approval required.
- [ ] Serialize mutation against built-in file tools for every planned target file.
- [ ] Return compact model output and structured diff details suitable for Pi rendering.

### Installation

- [ ] `deploy.sh` installs the Rust binaries and native Pi package.
- [ ] Installed Pi can load and execute the tool after reload or restart.

## How it works

- [Shared engine architecture](../wiki/systems/pi-regex-replace-tool.md)

## Implementation inventory

- `src/lib.rs` — shared search, replacement planning, validation, and writes.
- `src/main.rs` — MCP adapter.
- `src/bin/regex-replace-json.rs` — two-phase JSON request-file CLI adapter.
- `extensions/regex-replace.ts` — native Pi tool adapter.

## Tests asserting this spec

- Rust unit and integration tests under `src/` and `tests/`.
- TypeScript extension tests under `test/`.

## Known gaps (current cycle)

- [ ] Implement and verify the complete contract above.

## Out of scope

- A TypeScript reimplementation of regex matching or replacement semantics.
- Compatibility fallbacks for older Pi tool schemas.
- Publishing to npm or the public Pi package gallery.
