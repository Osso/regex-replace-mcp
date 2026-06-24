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
use std::path::{Path, PathBuf};

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

        let mut report = ReplaceReport::default();
        for path in files {
            process_replace_file(&path, &re, replacement.as_str(), dry_run, &mut report)?;
        }

        render_replace_report(report, dry_run)
    }

    fn do_search(&self, params: SearchParams) -> Result<String> {
        let re = Regex::new(&params.pattern).context("Invalid regex pattern")?;
        let limit = params.limit.unwrap_or(50);

        let files = collect_files(&params.files)?;
        if files.is_empty() {
            return Ok("No files matched the glob pattern.".to_string());
        }

        let mut report = SearchReport::default();
        for path in files {
            collect_file_matches(&path, &re, limit, &mut report);
            if report.matches.len() >= limit {
                break;
            }
        }

        render_search_report(report, limit)
    }
}

#[derive(Default)]
struct ReplaceReport {
    total_replacements: usize,
    files_modified: usize,
    output: String,
}

fn process_replace_file(
    path: &Path,
    re: &Regex,
    replacement: &str,
    dry_run: bool,
    report: &mut ReplaceReport,
) -> Result<()> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            report
                .output
                .push_str(&format!("Skipping {:?}: {}\n", path, error));
            return Ok(());
        }
    };

    let new_content = re.replace_all(&content, replacement);
    if new_content == content {
        return Ok(());
    }

    report.total_replacements += re.find_iter(&content).count();
    report.files_modified += 1;
    append_changed_lines(&mut report.output, path, &content, re, replacement);

    if dry_run {
        return Ok(());
    }
    fs::write(path, new_content.as_ref()).with_context(|| format!("Failed to write {:?}", path))?;
    Ok(())
}

fn append_changed_lines(
    output: &mut String,
    path: &Path,
    content: &str,
    re: &Regex,
    replacement: &str,
) {
    output.push_str(&format!("--- {}\n", path.display()));
    for (line_num, line) in content.lines().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        let replaced = re.replace_all(line, replacement);
        output.push_str(&format!("{}:- {}\n", line_num + 1, line));
        output.push_str(&format!("{}:+ {}\n", line_num + 1, replaced));
    }
    output.push('\n');
}

fn render_replace_report(report: ReplaceReport, dry_run: bool) -> Result<String> {
    if report.files_modified == 0 {
        return Ok("No matches found.".to_string());
    }

    let mut output = report.output;
    let mode = if dry_run { " (dry run)" } else { "" };
    output.push_str(&format!(
        "Total: {} replacement{} in {} file{}{}\n",
        report.total_replacements,
        plural_suffix(report.total_replacements),
        report.files_modified,
        plural_suffix(report.files_modified),
        mode
    ));
    Ok(output)
}

#[derive(Default)]
struct SearchReport {
    matches: Vec<String>,
    total_matches: usize,
}

fn collect_file_matches(path: &Path, re: &Regex, limit: usize, report: &mut SearchReport) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };

    for (line_num, line) in content.lines().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        report.total_matches += 1;
        if report.matches.len() >= limit {
            continue;
        }
        report.matches.push(format!(
            "{}:{}: {}",
            path.display(),
            line_num + 1,
            line.trim()
        ));
    }
}

fn render_search_report(report: SearchReport, limit: usize) -> Result<String> {
    if report.matches.is_empty() {
        return Ok("No matches found.".to_string());
    }

    let mut output = report.matches.join("\n");
    if report.total_matches > limit {
        output.push_str(&format!("\n\n... and more (showing first {})", limit));
    }
    output.push_str(&format!("\n\nTotal: {} matches", report.total_matches));
    Ok(output)
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Normalize replacement strings for the regex crate.
/// - `\n`, `\t`, `\r` are converted to actual newline, tab, carriage return
/// - `\\` is converted to a literal backslash
/// - `$1`, `$2` etc. become `${1}`, `${2}` to prevent ambiguity with following chars
/// - `$foo` becomes `$$foo` (escaped literal) since named capture groups are rarely intended
/// - `$$` stays as `$$` (already escaped literal)
fn escape_non_numeric_dollars(s: &str) -> String {
    let s = unescape_sequences(s);
    let mut result = String::with_capacity(s.len() * 2);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] != '$' {
            result.push(chars[i]);
            i += 1;
            continue;
        }
        i = escape_dollar_sequence(&chars, i, &mut result);
    }

    result
}

fn escape_dollar_sequence(chars: &[char], index: usize, result: &mut String) -> usize {
    let Some(next) = chars.get(index + 1).copied() else {
        result.push('$');
        return index + 1;
    };

    if next.is_ascii_digit() {
        return escape_numeric_dollar(chars, index, result);
    }
    if next == '$' {
        result.push_str("$$");
        return index + 2;
    }
    result.push_str("$$");
    index + 1
}

fn escape_numeric_dollar(chars: &[char], index: usize, result: &mut String) -> usize {
    let mut i = index + 1;
    result.push_str("${");
    while i < chars.len() && chars[i].is_ascii_digit() {
        result.push(chars[i]);
        i += 1;
    }
    result.push('}');
    i
}

/// Convert backslash escape sequences to their actual characters.
/// Handles: `\n` → newline, `\t` → tab, `\r` → carriage return, `\\` → backslash.
fn unescape_sequences(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
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

        // Backslash escape sequences in replacement strings
        assert_eq!(escape_non_numeric_dollars("line1\\nline2"), "line1\nline2");
        assert_eq!(escape_non_numeric_dollars("col1\\tcol2"), "col1\tcol2");
        assert_eq!(escape_non_numeric_dollars("a\\\\b"), "a\\b");
        assert_eq!(escape_non_numeric_dollars("$1\\n$2"), "${1}\n${2}");
    }

    #[test]
    fn test_unescape_sequences() {
        assert_eq!(unescape_sequences("hello\\nworld"), "hello\nworld");
        assert_eq!(unescape_sequences("a\\tb"), "a\tb");
        assert_eq!(unescape_sequences("a\\rb"), "a\rb");
        assert_eq!(unescape_sequences("a\\\\b"), "a\\b");
        // Unknown escapes preserved
        assert_eq!(unescape_sequences("a\\xb"), "a\\xb");
        // Trailing backslash preserved
        assert_eq!(unescape_sequences("foo\\"), "foo\\");
        // No escapes
        assert_eq!(unescape_sequences("plain text"), "plain text");
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
    fn test_replace_with_newline_in_replacement() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.rs", "    field_a: bool,\n    }");

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: r"(field_a: bool,)".to_string(),
                replacement: "$1\\n    field_b: f64,".to_string(),
                files: dir.path().join("*.rs").to_string_lossy().to_string(),
                dry_run: Some(false),
            })
            .unwrap();

        assert!(result.contains("1 replacement"));

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "    field_a: bool,\n    field_b: f64,\n    }");
    }

    #[test]
    fn test_replace_with_tab_in_replacement() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "col1,col2");

        let service = RegexReplaceService::new();
        service
            .do_replace(ReplaceParams {
                pattern: ",".to_string(),
                replacement: "\\t".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: Some(false),
            })
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "col1\tcol2");
    }

    #[test]
    fn test_replace_literal_backslash_in_replacement() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "forward/slash");

        let service = RegexReplaceService::new();
        service
            .do_replace(ReplaceParams {
                pattern: "/".to_string(),
                replacement: "\\\\".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: Some(false),
            })
            .unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "forward\\slash");
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

    #[test]
    fn test_search_limit_reports_hidden_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "a.txt", "match one\nmatch two");
        create_test_file(&dir, "b.txt", "match three\nmatch four");

        let service = RegexReplaceService::new();
        let result = service
            .do_search(SearchParams {
                pattern: "match".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: Some(1),
            })
            .unwrap();

        assert!(result.contains("match one"));
        assert!(result.contains("... and more (showing first 1)"));
        assert!(result.contains("Total: 2 matches"));
        assert!(!result.contains("match three"));
    }

    #[test]
    fn test_replace_no_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world");

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: "missing".to_string(),
                replacement: "found".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: None,
            })
            .unwrap();

        assert_eq!(result, "No matches found.");
    }

    #[test]
    fn test_replace_no_files_matched() {
        let dir = TempDir::new().unwrap();

        let service = RegexReplaceService::new();
        let result = service
            .do_replace(ReplaceParams {
                pattern: "test".to_string(),
                replacement: "found".to_string(),
                files: dir.path().join("*.xyz").to_string_lossy().to_string(),
                dry_run: None,
            })
            .unwrap();

        assert_eq!(result, "No files matched the glob pattern.");
    }

    #[test]
    fn test_collect_files_skips_directories() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world");
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let files = collect_files(&dir.path().join("*").to_string_lossy()).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("test.txt"));
    }

    #[test]
    fn test_replace_file_records_read_errors() {
        let missing_path = PathBuf::from("/tmp/regex-replace-mcp-missing-file");
        let re = Regex::new("test").unwrap();
        let mut report = ReplaceReport::default();

        process_replace_file(&missing_path, &re, "done", false, &mut report).unwrap();

        assert_eq!(report.total_replacements, 0);
        assert_eq!(report.files_modified, 0);
        assert!(report.output.contains("Skipping"));
        assert!(report.output.contains("regex-replace-mcp-missing-file"));
    }

    #[test]
    fn test_collect_file_matches_ignores_read_errors() {
        let missing_path = PathBuf::from("/tmp/regex-replace-mcp-missing-search-file");
        let re = Regex::new("test").unwrap();
        let mut report = SearchReport::default();

        collect_file_matches(&missing_path, &re, 10, &mut report);

        assert!(report.matches.is_empty());
        assert_eq!(report.total_matches, 0);
    }

    #[test]
    fn test_server_info_enables_tools() {
        let service = RegexReplaceService::new();
        let info = service.get_info();

        assert!(
            info.instructions
                .unwrap()
                .contains("Regex find-and-replace MCP server")
        );
        assert!(info.capabilities.tools.is_some());
    }

    #[tokio::test]
    async fn test_tool_wrappers_return_success_and_errors() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world");

        let service = RegexReplaceService::new();
        let search_result = service
            .regex_search(Parameters(SearchParams {
                pattern: "hello".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: None,
            }))
            .await;
        let replace_error = service
            .regex_replace(Parameters(ReplaceParams {
                pattern: "(".to_string(),
                replacement: "x".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                dry_run: Some(true),
            }))
            .await;

        assert!(search_result.contains("hello world"));
        assert!(replace_error.contains("Error: Invalid regex pattern"));
    }
}
