# regex-replace-mcp

MCP server for regex find-and-replace across files.

## Build

```bash
cargo build --release
```

## Test

```bash
cargo test
```

## Key Implementation Details

- `escape_non_numeric_dollars()`: Escapes `$` in replacement strings except when followed by digits. This prevents `$request` from being interpreted as a named capture group (which would become empty).
