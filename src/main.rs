//! MCP server for regex find-and-replace across files.

use anyhow::{Context, Result};
use glob::glob;
use regex::Regex;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Clone)]
pub struct RegexReplaceService {
    tool_router: ToolRouter<Self>,
}

impl RegexReplaceService {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplaceParams {
    /// Regex pattern to match
    #[schemars(description = "Regex pattern to match (Rust regex syntax)")]
    pattern: String,

    /// Replacement string (use $1, $2 for capture groups, $0 for entire match)
    #[schemars(description = "Replacement string. Use $1, $2 for capture groups, $0 for entire match")]
    replacement: String,

    /// Glob pattern for files to process (e.g., "src/**/*.php")
    #[schemars(description = "Glob pattern for files (e.g., 'src/**/*.php')")]
    files: String,

    /// Preview changes without writing (default: false)
    #[schemars(description = "Preview changes without writing (default: false)")]
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Regex pattern to search for
    #[schemars(description = "Regex pattern to search for (Rust regex syntax)")]
    pattern: String,

    /// Glob pattern for files to search (e.g., "src/**/*.php")
    #[schemars(description = "Glob pattern for files (e.g., 'src/**/*.php')")]
    files: String,

    /// Maximum matches to return (default: 50)
    #[schemars(description = "Maximum matches to return (default: 50)")]
    limit: Option<usize>,
}

#[tool_router]
impl RegexReplaceService {
    #[tool(
        description = "Replace text matching a regex pattern across multiple files. Supports capture groups ($1, $2, etc.) in replacement. Returns a summary of changes made."
    )]
    async fn regex_replace(&self, Parameters(params): Parameters<ReplaceParams>) -> String {
        match self.do_replace(params) {
            Ok(msg) => msg,
            Err(e) => format!("Error: {}", e),
        }
    }

    #[tool(
        description = "Search for regex pattern matches across files. Returns matching lines with file paths and line numbers."
    )]
    async fn regex_search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        match self.do_search(params) {
            Ok(msg) => msg,
            Err(e) => format!("Error: {}", e),
        }
    }
}

impl RegexReplaceService {
    fn do_replace(&self, params: ReplaceParams) -> Result<String> {
        let re = Regex::new(&params.pattern).context("Invalid regex pattern")?;
        let dry_run = params.dry_run.unwrap_or(false);
        let replacement = escape_non_numeric_dollars(&params.replacement);

        let files = collect_files(&params.files)?;
        if files.is_empty() {
            return Ok("No files matched the glob pattern.".to_string());
        }

        let mut total_replacements = 0;
        let mut files_modified = 0;
        let mut output = String::new();

        for path in files {
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    output.push_str(&format!("Skipping {:?}: {}\n", path, e));
                    continue;
                }
            };

            let new_content = re.replace_all(&content, replacement.as_str());

            if new_content != content {
                let count = re.find_iter(&content).count();
                total_replacements += count;
                files_modified += 1;

                output.push_str(&format!("--- {}\n", path.display()));
                // Show each line that changed
                for (line_num, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        let replaced = re.replace_all(line, replacement.as_str());
                        output.push_str(&format!("{}:- {}\n", line_num + 1, line));
                        output.push_str(&format!("{}:+ {}\n", line_num + 1, replaced));
                    }
                }
                output.push('\n');

                if !dry_run {
                    fs::write(&path, new_content.as_ref())
                        .with_context(|| format!("Failed to write {:?}", path))?;
                }
            }
        }

        if files_modified == 0 {
            Ok("No matches found.".to_string())
        } else {
            let mode = if dry_run { " (dry run)" } else { "" };
            output.push_str(&format!(
                "Total: {} replacement{} in {} file{}{}\n",
                total_replacements,
                if total_replacements == 1 { "" } else { "s" },
                files_modified,
                if files_modified == 1 { "" } else { "s" },
                mode
            ));
            Ok(output)
        }
    }

    fn do_search(&self, params: SearchParams) -> Result<String> {
        let re = Regex::new(&params.pattern).context("Invalid regex pattern")?;
        let limit = params.limit.unwrap_or(50);

        let files = collect_files(&params.files)?;
        if files.is_empty() {
            return Ok("No files matched the glob pattern.".to_string());
        }

        let mut matches = Vec::new();
        let mut total_matches = 0;

        'outer: for path in files {
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    total_matches += 1;
                    if matches.len() < limit {
                        matches.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            line_num + 1,
                            line.trim()
                        ));
                    }
                    if matches.len() >= limit {
                        break 'outer;
                    }
                }
            }
        }

        if matches.is_empty() {
            Ok("No matches found.".to_string())
        } else {
            let mut output = matches.join("\n");
            if total_matches > limit {
                output.push_str(&format!("\n\n... and more (showing first {})", limit));
            }
            output.push_str(&format!("\n\nTotal: {} matches", total_matches));
            Ok(output)
        }
    }
}

/// Escape `$` in replacement strings except when followed by a digit (capture group reference).
/// This prevents `$foo` from being treated as a named capture group (which would become empty).
fn escape_non_numeric_dollars(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.peek() {
                // $0, $1, $2, etc. - capture group reference, keep as-is
                Some(&next) if next.is_ascii_digit() => result.push('$'),
                // $$ - already escaped literal $, keep both and consume second $
                Some(&'$') => {
                    result.push_str("$$");
                    chars.next();
                }
                // $foo - escape by doubling the $
                Some(_) => result.push_str("$$"),
                // Trailing $ - keep as-is
                None => result.push('$'),
            }
        } else {
            result.push(c);
        }
    }

    result
}

fn collect_files(pattern: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in glob(pattern).context("Invalid glob pattern")? {
        match entry {
            Ok(path) if path.is_file() => files.push(path),
            Ok(_) => {} // Skip directories
            Err(e) => eprintln!("Glob error: {}", e),
        }
    }
    Ok(files)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RegexReplaceService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Regex find-and-replace MCP server. Use regex_replace for replacements, regex_search for searching."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let service = RegexReplaceService::new();
    let server = service.serve(stdio()).await?;
    server.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_non_numeric_dollars() {
        // Capture groups should be preserved
        assert_eq!(escape_non_numeric_dollars("$1"), "$1");
        assert_eq!(escape_non_numeric_dollars("$0"), "$0");
        assert_eq!(escape_non_numeric_dollars("$1$2"), "$1$2");

        // Already escaped $$ should be preserved
        assert_eq!(escape_non_numeric_dollars("$$"), "$$");
        assert_eq!(escape_non_numeric_dollars("$$foo"), "$$foo");

        // $name should be escaped to $$name
        assert_eq!(escape_non_numeric_dollars("$request"), "$$request");
        assert_eq!(
            escape_non_numeric_dollars("$request->get"),
            "$$request->get"
        );

        // Mixed cases
        assert_eq!(
            escape_non_numeric_dollars("$request->get->getInt('$1', $2)"),
            "$$request->get->getInt('$1', $2)"
        );

        // Trailing $ should be preserved
        assert_eq!(escape_non_numeric_dollars("foo$"), "foo$");

        // No $ at all
        assert_eq!(escape_non_numeric_dollars("hello"), "hello");
    }
}
