# regex-replace-mcp

[![CI](https://github.com/Osso/regex-replace-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/Osso/regex-replace-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Shared Rust engine for safe regex find-and-replace across files, exposed through MCP, a two-phase JSON CLI, and a native Pi tool.

## Installation

Run the project deploy script:

```bash
./deploy.sh
```

It builds and installs both Rust binaries and registers this repository as a local Pi package.

For MCP-only installation:

```bash
cargo install --git https://github.com/Osso/regex-replace-mcp
```

## MCP Configuration

Add to your Claude Code MCP config (`~/.claude.json`):

```json
{
  "mcpServers": {
    "regex-replace": {
      "type": "stdio",
      "command": "/path/to/regex-replace-mcp",
      "args": []
    }
  }
}
```

## Tools

### regex_search

Search for regex pattern matches across files.

Parameters:
- `pattern`: Regex pattern (Rust regex syntax)
- `files`: Glob pattern for files (e.g., `src/**/*.rs`)
- `limit`: Maximum matches to return (default: 50)

### regex_replace

Replace text matching a regex pattern across multiple files.

Parameters:
- `pattern`: Regex pattern (Rust regex syntax)
- `replacement`: Replacement string with capture group support
- `files`: Glob pattern for files
- `dry_run`: Preview changes without writing (default: false)

## Native Pi Tool

The `regex_replace` tool accepts:

- `files`: gitignore-aware glob relative to the current working directory.
- `pattern`: Rust regex syntax.
- `replacement`: replacement string with `$1`, `$2`, and `$0` captures.
- `expectedMatches`: required exact match count.
- `dryRun`: preview without writing.
- Optional file, byte, and match limits.

Non-dry runs use a two-phase protocol. Pi previews the plan, locks every canonical target through its file-mutation queue, and applies only if the frozen targets still produce the same plan hash. Each file is replaced atomically; multi-file failures use transaction-like rollback rather than claiming literal cross-file atomic visibility.

## Capture Groups

Use `$1`, `$2`, etc. for capture groups in replacements:

```
pattern: "fn (\w+)\(\)"
replacement: "fn $1_v2()"
```

Literal `$` in replacements (like `$request`) stays literal - only `$` followed by digits is treated as a capture group reference.

## License

MIT
