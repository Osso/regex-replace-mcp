use anyhow::{Context, Result, bail};
use regex_replace_mcp::{ReplaceLimits, ReplaceRequest, replace};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Action {
    Plan,
    Apply,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsonRequest {
    action: Action,
    cwd: PathBuf,
    files: String,
    pattern: String,
    replacement: String,
    expected_matches: usize,
    plan_hash: Option<String>,
    targets: Option<Vec<PathBuf>>,
    max_files: usize,
    max_total_bytes: u64,
    max_matches: usize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let request_path = request_path()?;
    let request_json = fs::read_to_string(&request_path)
        .with_context(|| format!("Failed to read request file {}", request_path.display()))?;
    let input: JsonRequest = serde_json::from_str(&request_json)
        .with_context(|| format!("Invalid request file {}", request_path.display()))?;
    let request = input.into_replace_request()?;
    let result = replace(request)?;
    serde_json::to_writer(std::io::stdout(), &result).context("Failed to write JSON result")?;
    Ok(())
}

fn request_path() -> Result<PathBuf> {
    let mut arguments = std::env::args_os();
    let _binary = arguments.next();
    let Some(path) = arguments.next() else {
        bail!("Usage: regex-replace-json <request.json>");
    };
    if arguments.next().is_some() {
        bail!("Usage: regex-replace-json <request.json>");
    }
    Ok(PathBuf::from(path))
}

impl JsonRequest {
    fn into_replace_request(self) -> Result<ReplaceRequest> {
        if self.expected_matches == 0 {
            bail!("expectedMatches must be greater than zero");
        }
        let limits = ReplaceLimits {
            max_files: self.max_files,
            max_total_bytes: self.max_total_bytes,
            max_matches: self.max_matches,
        };
        match self.action {
            Action::Plan => {
                if self.plan_hash.is_some() || self.targets.is_some() {
                    bail!("plan requests must not include planHash or targets");
                }
                Ok(ReplaceRequest {
                    cwd: self.cwd,
                    files: self.files,
                    pattern: self.pattern,
                    replacement: self.replacement,
                    dry_run: true,
                    expected_matches: Some(self.expected_matches),
                    expected_plan_hash: None,
                    target_files: None,
                    limits,
                })
            }
            Action::Apply => {
                let plan_hash = self.plan_hash.context("apply requests require planHash")?;
                let targets = self.targets.context("apply requests require targets")?;
                if targets.is_empty() {
                    bail!("apply requests require at least one target");
                }
                Ok(ReplaceRequest {
                    cwd: self.cwd,
                    files: self.files,
                    pattern: self.pattern,
                    replacement: self.replacement,
                    dry_run: false,
                    expected_matches: Some(self.expected_matches),
                    expected_plan_hash: Some(plan_hash),
                    target_files: Some(targets),
                    limits,
                })
            }
        }
    }
}
