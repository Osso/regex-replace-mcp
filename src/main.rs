//! MCP server for regex find-and-replace across files.

use anyhow::{Context, Result};
use glob::glob;
use regex::Regex;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
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
    #[schemars(
        description = "Replacement string. Use $1, $2 for capture groups, $0 for entire match"
    )]
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

/// Normalize replacement strings for the regex crate.
/// - `$1`, `$2` etc. become `${1}`, `${2}` to prevent ambiguity with following chars
/// - `$foo` becomes `$$foo` (escaped literal) since named capture groups are rarely intended
/// - `$$` stays as `$$` (already escaped literal)
fn escape_non_numeric_dollars(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '$' {
            if i + 1 < chars.len() {
                let next = chars[i + 1];
                if next.is_ascii_digit() {
                    // $0, $1, $2, etc. - collect all digits and wrap in ${N}
                    result.push_str("${");
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        result.push(chars[i]);
                        i += 1;
                    }
                    result.push('}');
                    continue;
                } else if next == '$' {
                    // $$ - already escaped literal $, keep both
                    result.push_str("$$");
                    i += 2;
                    continue;
                } else {
                    // $foo - escape by doubling the $
                    result.push_str("$$");
                    i += 1;
                    continue;
                }
            } else {
                // Trailing $ - keep as-is
                result.push('$');
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
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
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_escape_non_numeric_dollars() {
        // Capture groups get wrapped in ${N} to prevent ambiguity
        assert_eq!(escape_non_numeric_dollars("$1"), "${1}");
        assert_eq!(escape_non_numeric_dollars("$0"), "${0}");
        assert_eq!(escape_non_numeric_dollars("$1$2"), "${1}${2}");
        assert_eq!(escape_non_numeric_dollars("$12"), "${12}");

        // Capture groups followed by text work correctly
        assert_eq!(escape_non_numeric_dollars("$1_v2"), "${1}_v2");
        assert_eq!(escape_non_numeric_dollars("fn $1()"), "fn ${1}()");

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
            "$$request->get->getInt('${1}', ${2})"
        );

        // Trailing $ should be preserved
        assert_eq!(escape_non_numeric_dollars("foo$"), "foo$");

        // No $ at all
        assert_eq!(escape_non_numeric_dollars("hello"), "hello");
    }

    fn create_test_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_search_finds_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world\nfoo bar\nhello again");

        let service = RegexReplaceService::new();
        let result = service
            .do_search(SearchParams {
                pattern: "hello".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: None,
            })
            .unwrap();

        assert!(result.contains("hello world"));
        assert!(result.contains("hello again"));
        assert!(result.contains("Total: 2 matches"));
    }

    #[test]
    fn test_search_no_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world");

        let service = RegexReplaceService::new();
        let result = service
            .do_search(SearchParams {
                pattern: "xyz".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: None,
            })
            .unwrap();

        assert_eq!(result, "No matches found.");
    }

    #[test]
    fn test_replace_with_capture_groups() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "fn hello() {}\nfn world() {}");

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: r"fn (\w+)\(\)".to_string(),
                replacement: "fn $1_v2()".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: Some(false),
            })
            .unwrap();

        assert!(result.contains("2 replacements"));

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("fn hello_v2()"));
        assert!(content.contains("fn world_v2()"));
    }

    #[test]
    fn test_replace_preserves_dollar_variables() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(
            &dir,
            "test.php",
            "$page = intval(array_get($request->get, 'p', 1));",
        );

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: r"intval\(array_get\(\$request->get, '([^']+)', (\d+)\)\)".to_string(),
                replacement: "$request->get->getInt('$1', $2)".to_string(),
                files: dir.path().join("*.php").to_string_lossy().to_string(),
                dry_run: Some(false),
            })
            .unwrap();

        assert!(result.contains("1 replacement"));

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "$page = $request->get->getInt('p', 1);");
    }

    #[test]
    fn test_replace_dry_run() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "hello world");

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: "hello".to_string(),
                replacement: "goodbye".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: Some(true),
            })
            .unwrap();

        assert!(result.contains("(dry run)"));

        // File should be unchanged
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn test_no_files_matched() {
        let dir = TempDir::new().unwrap();

        let service = RegexReplaceService::new();
        let result = service
            .do_search(SearchParams {
                pattern: "test".to_string(),
                files: dir.path().join("*.xyz").to_string_lossy().to_string(),
                limit: None,
            })
            .unwrap();

        assert_eq!(result, "No files matched the glob pattern.");
    }
}
