# Pi Regex Replace Tool

The native Pi tool is a TypeScript adapter over the repository's Rust replacement engine. Regex matching, glob expansion, validation, diff generation, and writes remain in Rust so MCP and Pi share the same semantics.

## Flow

1. Pi calls `regex-replace-json` with a temporary JSON `plan` request.
2. Rust expands the glob with `.gitignore` filtering, reads every target, validates limits and the exact expected match count, and returns changed files as sorted canonical targets, original hashes, unified diffs, and a deterministic plan hash.
3. The extension acquires Pi's per-file mutation queue for every canonical target in sorted order.
4. Pi calls the CLI with an `apply` request containing the approved plan hash and frozen target list.
5. Rust rereads only those targets, recomputes the plan, rejects drift before writing, stages every output beside its target, and replaces files with atomic per-file renames.
6. If a later rename fails, Rust attempts to restore files already replaced and reports rollback failure explicitly.

The write protocol provides stale-plan rejection, atomic replacement per file, and transaction-like rollback. It cannot provide literal simultaneous atomic visibility across multiple filesystem paths.

## Deployment

`deploy.sh` builds release binaries, installs `regex-replace-mcp` and `regex-replace-json` under `$HOME/.local/bin`, and runs `pi install` for this repository. The package metadata exposes `./extensions/regex-replace.ts` as the native extension. Restart Pi after deployment to load it; the package is private and is not published to npm or the public Pi package gallery.

## Boundaries

- `src/lib.rs` owns all replacement semantics and filesystem safety.
- `src/main.rs` maps MCP replacement requests to the shared engine, keeps search behavior, and preserves legacy human-readable output.
- `src/bin/regex-replace-json.rs` maps JSON request files to the engine.
- `src/pi/regex-replace-tool.ts` owns plan/apply orchestration and sorted nested file queues.
- `src/pi/json-cli-runner.ts` owns temporary request-file lifecycle and process result parsing.
- `extensions/regex-replace.ts` owns Pi schema, approval metadata, rendering, and bounded model output.

## Dependency rationale

Rust dependencies are scoped to current requirements:

- `ignore` and `globset` provide gitignore-aware file selection.
- `similar` produces unified diffs.
- `sha2` produces deterministic content and plan hashes.
- `tempfile` provides same-directory staged writes.

Pi packages are peer dependencies at runtime. The npm development dependencies exist only for extension tests and type checking and are not bundled into the installed Pi package.
