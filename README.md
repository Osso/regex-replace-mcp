# regex-replace-mcp

[![CI](https://github.com/Osso/regex-replace-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/Osso/regex-replace-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

MCP server for regex find-and-replace across files. Designed for use with Claude Code and other MCP clients.

## Installation

```bash
cargo install --git https://github.com/Osso/regex-replace-mcp
```

Or build from source:

```bash
git clone https://github.com/Osso/regex-replace-mcp
cd regex-replace-mcp
cargo build --release
```

## Configuration

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

## Capture Groups

Use `$1`, `$2`, etc. for capture groups in replacements:

```
pattern: "fn (\w+)\(\)"
replacement: "fn $1_v2()"
```

Literal `$` in replacements (like `$request`) stays literal - only `$` followed by digits is treated as a capture group reference.

## License

MIT
