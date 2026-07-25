//! MCP server for regex find-and-replace across files.

use anyhow::{Context, Result};
use glob::glob;
use regex::Regex;
use regex_replace_mcp::{ReplaceLimits, ReplaceRequest, ReplaceResult, replace};
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
    #[schemars(description = "Regex pattern to match (Rust regex syntax)")]
    pattern: String,

    #[schemars(
        description = "Replacement string. Use $1, $2 for capture groups, $0 for entire match"
    )]
    replacement: String,

    #[schemars(description = "Glob pattern for files (e.g., 'src/**/*.php')")]
    files: String,

    #[schemars(description = "Preview changes without writing (default: false)")]
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Regex pattern to search for (Rust regex syntax)")]
    pattern: String,

    #[schemars(description = "Glob pattern for files (e.g., 'src/**/*.php')")]
    files: String,

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
            Ok(message) => message,
            Err(error) => format!("Error: {error}"),
        }
    }

    #[tool(
        description = "Search for regex pattern matches across files. Returns matching lines with file paths and line numbers."
    )]
    async fn regex_search(&self, Parameters(params): Parameters<SearchParams>) -> String {
        match self.do_search(params) {
            Ok(message) => message,
            Err(error) => format!("Error: {error}"),
        }
    }
}

impl RegexReplaceService {
    fn do_replace(&self, params: ReplaceParams) -> Result<String> {
        let dry_run = params.dry_run.unwrap_or(false);
        let request = ReplaceRequest {
            cwd: std::env::current_dir().context("Failed to read current directory")?,
            files: params.files,
            pattern: params.pattern,
            replacement: params.replacement,
            dry_run,
            expected_matches: None,
            expected_plan_hash: None,
            target_files: None,
            limits: ReplaceLimits::default(),
        };
        let result = replace(request)?;
        Ok(render_replace_result(result))
    }

    fn do_search(&self, params: SearchParams) -> Result<String> {
        let regex = Regex::new(&params.pattern).context("Invalid regex pattern")?;
        let limit = params.limit.unwrap_or(50);
        let files = collect_search_files(&params.files)?;
        if files.is_empty() {
            return Ok("No files matched the glob pattern.".to_string());
        }

        let mut report = SearchReport::default();
        for path in files {
            collect_file_matches(&path, &regex, limit, &mut report);
            if report.matches.len() >= limit {
                break;
            }
        }
        Ok(render_search_report(report, limit))
    }
}

fn render_replace_result(result: ReplaceResult) -> String {
    if result.matched_files == 0 {
        return "No files matched the glob pattern.".to_string();
    }
    if result.files_modified == 0 {
        return "No matches found.".to_string();
    }

    let mut output = String::new();
    for change in &result.changes {
        output.push_str(&format!("--- {}\n", change.path));
        for line in &change.line_changes {
            output.push_str(&format!("{}:- {}\n", line.line_number, line.before));
            output.push_str(&format!("{}:+ {}\n", line.line_number, line.after));
        }
        output.push('\n');
    }

    let mode = if result.dry_run { " (dry run)" } else { "" };
    output.push_str(&format!(
        "Total: {} replacement{} in {} file{}{}\n",
        result.total_replacements,
        plural_suffix(result.total_replacements),
        result.files_modified,
        plural_suffix(result.files_modified),
        mode
    ));
    output
}

#[derive(Default)]
struct SearchReport {
    matches: Vec<String>,
    total_matches: usize,
}

fn collect_file_matches(path: &PathBuf, regex: &Regex, limit: usize, report: &mut SearchReport) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    for (line_index, line) in content.lines().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        report.total_matches += 1;
        if report.matches.len() < limit {
            report.matches.push(format!(
                "{}:{}: {}",
                path.display(),
                line_index + 1,
                line.trim()
            ));
        }
    }
}

fn render_search_report(report: SearchReport, limit: usize) -> String {
    if report.matches.is_empty() {
        return "No matches found.".to_string();
    }
    let mut output = report.matches.join("\n");
    if report.total_matches > limit {
        output.push_str(&format!("\n\n... and more (showing first {limit})"));
    }
    output.push_str(&format!("\n\nTotal: {} matches", report.total_matches));
    output
}

fn collect_search_files(pattern: &str) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in glob(pattern).context("Invalid glob pattern")? {
        match entry {
            Ok(path) if path.is_file() => files.push(path),
            Ok(_) => {}
            Err(error) => eprintln!("Glob error: {error}"),
        }
    }
    Ok(files)
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
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

    fn create_test_file(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    fn replace_params(dir: &TempDir, pattern: &str, replacement: &str) -> ReplaceParams {
        ReplaceParams {
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            files: dir.path().join("*.txt").to_string_lossy().to_string(),
            dry_run: Some(false),
        }
    }

    #[test]
    fn search_finds_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world\nfoo bar\nhello again");
        let result = RegexReplaceService::new()
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
    fn search_no_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "test.txt", "hello world");
        let result = RegexReplaceService::new()
            .do_search(SearchParams {
                pattern: "xyz".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: None,
            })
            .unwrap();
        assert_eq!(result, "No matches found.");
    }

    #[test]
    fn replace_with_capture_groups() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "fn hello() {}\nfn world() {}");
        let result = RegexReplaceService::new()
            .do_replace(replace_params(&dir, r"fn (\w+)\(\)", "fn $1_v2()"))
            .unwrap();
        assert!(result.contains("2 replacements"));
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("fn hello_v2()"));
        assert!(content.contains("fn world_v2()"));
    }

    #[test]
    fn replace_preserves_dollar_variables() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(
            &dir,
            "test.txt",
            "$page = intval(array_get($request->get, 'p', 1));",
        );
        let result = RegexReplaceService::new()
            .do_replace(replace_params(
                &dir,
                r"intval\(array_get\(\$request->get, '([^']+)', (\d+)\)\)",
                "$request->get->getInt('$1', $2)",
            ))
            .unwrap();
        assert!(result.contains("1 replacement"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "$page = $request->get->getInt('p', 1);"
        );
    }

    #[test]
    fn replace_dry_run_does_not_write() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "hello world");
        let mut params = replace_params(&dir, "hello", "goodbye");
        params.dry_run = Some(true);
        let result = RegexReplaceService::new().do_replace(params).unwrap();
        assert!(result.contains("(dry run)"));
        assert_eq!(fs::read_to_string(path).unwrap(), "hello world");
    }

    #[test]
    fn replacement_escape_sequences_work() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "test.txt", "field_a: bool,\n}");
        RegexReplaceService::new()
            .do_replace(replace_params(
                &dir,
                r"(field_a: bool,)",
                "$1\\nfield_b: f64,",
            ))
            .unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "field_a: bool,\nfield_b: f64,\n}"
        );
    }

    #[test]
    fn no_files_and_no_matches_have_legacy_messages() {
        let dir = TempDir::new().unwrap();
        let service = RegexReplaceService::new();
        let no_files = service
            .do_replace(ReplaceParams {
                pattern: "test".to_string(),
                replacement: "done".to_string(),
                files: dir.path().join("*.missing").to_string_lossy().to_string(),
                dry_run: None,
            })
            .unwrap();
        create_test_file(&dir, "test.txt", "hello world");
        let no_matches = service
            .do_replace(replace_params(&dir, "missing", "found"))
            .unwrap();
        assert_eq!(no_files, "No files matched the glob pattern.");
        assert_eq!(no_matches, "No matches found.");
    }

    #[test]
    fn search_limit_reports_hidden_matches() {
        let dir = TempDir::new().unwrap();
        create_test_file(&dir, "a.txt", "match one\nmatch two");
        create_test_file(&dir, "b.txt", "match three\nmatch four");
        let result = RegexReplaceService::new()
            .do_search(SearchParams {
                pattern: "match".to_string(),
                files: dir.path().join("*.txt").to_string_lossy().to_string(),
                limit: Some(1),
            })
            .unwrap();
        assert!(result.contains("match one"));
        assert!(result.contains("... and more (showing first 1)"));
        assert!(result.contains("Total: 2 matches"));
    }

    #[test]
    fn server_info_enables_tools() {
        let info = RegexReplaceService::new().get_info();
        assert!(
            info.instructions
                .unwrap()
                .contains("Regex find-and-replace MCP server")
        );
        assert!(info.capabilities.tools.is_some());
    }

    #[tokio::test]
    async fn tool_wrappers_return_success_and_errors() {
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
