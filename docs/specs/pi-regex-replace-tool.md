# Pi Regex Replace Tool

The Pi regex replacement tool exposes the repository's Rust replacement engine through a native Pi extension. The shared engine also remains available to the MCP server and JSON CLI.

## What it must do

### Shared replacement behavior

- [x] Preserve existing Rust-regex capture-group, literal-dollar, and escape-sequence behavior.
- [x] Plan all matched-file changes before writing.
- [x] Require an exact expected match count for Pi/JSON requests and reject mismatches without modifying files.
- [x] For Pi/JSON requests, reject invalid regexes, binary/non-UTF-8 inputs, zero actual matches, and configured file, byte, or match limit violations without modifying files.
- [x] Respect `.gitignore` while expanding file globs.
- [x] Return a stable plan hash and reject application when the files no longer match the approved plan.
- [x] Produce unified diffs for every changed file in dry-run and applied results.
- [x] Replace each file with an atomic rename and roll back already-written files if a later rename fails. This is transaction-like recovery, not literal cross-file atomic visibility.

### Interfaces

- [x] Preserve existing MCP search and replacement behavior.
- [x] Provide a two-phase JSON request-file CLI for planning and applying frozen replacement targets.
- [x] Register a native Pi `regex_replace` tool with strict parameters and approval required.
- [x] Instruct model callers to omit optional safety limits unless the user explicitly requests non-default limits.
- [x] Serialize mutation against built-in file tools for every planned target file.
- [x] Return compact model output and native Pi result details with `dryRun`, `totalReplacements`, and `filesModified` plus a bounded `diff`, keeping serialized details below Pi's 10 KiB runtime cap.
- [x] Explicitly mark oversized diffs as truncated in result details and model output rather than allowing Pi to render `Invalid replacement details`.

### Installation

- [x] `deploy.sh` builds and installs the Rust binaries and registers the repository as a local Pi package.
- [x] Installed Pi loads the tool after reload or restart.

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

## Current status

The native Pi extension and its Rust JSON CLI backend are implemented and deployed as a local Pi package. Public npm or Pi gallery publication remains out of scope.

## Out of scope

- A TypeScript reimplementation of regex matching or replacement semantics.
- Compatibility fallbacks for older Pi tool schemas.
- Publishing to npm or the public Pi package gallery.
