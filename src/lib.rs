use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use tempfile::NamedTempFile;

const DEFAULT_MAX_FILES: usize = 100;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_MAX_MATCHES: usize = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceLimits {
    pub max_files: usize,
    pub max_total_bytes: u64,
    pub max_matches: usize,
}

impl Default for ReplaceLimits {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_matches: DEFAULT_MAX_MATCHES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplaceRequest {
    pub cwd: PathBuf,
    pub files: String,
    pub pattern: String,
    pub replacement: String,
    pub dry_run: bool,
    pub expected_matches: Option<usize>,
    pub expected_plan_hash: Option<String>,
    pub target_files: Option<Vec<PathBuf>>,
    pub limits: ReplaceLimits,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineChange {
    pub line_number: usize,
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub absolute_path: String,
    pub original_hash: String,
    pub replacements: usize,
    pub diff: String,
    pub line_changes: Vec<LineChange>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceResult {
    pub dry_run: bool,
    pub plan_hash: String,
    pub matched_files: usize,
    pub total_replacements: usize,
    pub files_modified: usize,
    pub changes: Vec<FileChange>,
}

struct PlannedFile {
    canonical_path: PathBuf,
    display_path: String,
    original_hash: String,
    original: String,
    replacement: String,
    permissions: fs::Permissions,
    replacements: usize,
    diff: String,
    line_changes: Vec<LineChange>,
}

struct ReplacePlan {
    matched_files: usize,
    total_replacements: usize,
    hash: String,
    files: Vec<PlannedFile>,
}

struct PlanBuilder<'a> {
    request: &'a ReplaceRequest,
    regex: &'a Regex,
    replacement: &'a str,
    total_bytes: u64,
    total_replacements: usize,
    files: Vec<PlannedFile>,
}

pub fn replace(request: ReplaceRequest) -> Result<ReplaceResult> {
    validate_limits(&request.limits)?;
    let regex = Regex::new(&request.pattern).context("Invalid regex pattern")?;
    let replacement = escape_non_numeric_dollars(&request.replacement);
    let plan = plan_replacement(&request, &regex, &replacement)?;

    validate_expected_matches(request.expected_matches, plan.total_replacements)?;
    validate_expected_plan_hash(request.expected_plan_hash.as_deref(), &plan.hash)?;

    if !request.dry_run {
        apply_plan(&plan)?;
    }

    Ok(render_result(plan, request.dry_run))
}

fn validate_limits(limits: &ReplaceLimits) -> Result<()> {
    if limits.max_files == 0 {
        bail!("max_files must be greater than zero");
    }
    if limits.max_total_bytes == 0 {
        bail!("max_total_bytes must be greater than zero");
    }
    if limits.max_matches == 0 {
        bail!("max_matches must be greater than zero");
    }
    Ok(())
}

fn plan_replacement(
    request: &ReplaceRequest,
    regex: &Regex,
    replacement: &str,
) -> Result<ReplacePlan> {
    let paths = request_paths(request)?;
    let matched_files = paths.len();
    let mut builder = PlanBuilder::new(request, regex, replacement);
    for path in paths {
        builder.add_file(path)?;
    }
    Ok(builder.finish(matched_files))
}

impl<'a> PlanBuilder<'a> {
    fn new(request: &'a ReplaceRequest, regex: &'a Regex, replacement: &'a str) -> Self {
        Self {
            request,
            regex,
            replacement,
            total_bytes: 0,
            total_replacements: 0,
            files: Vec::new(),
        }
    }

    fn add_file(&mut self, canonical_path: PathBuf) -> Result<()> {
        let bytes = fs::read(&canonical_path)
            .with_context(|| format!("Failed to read {}", canonical_path.display()))?;
        self.record_bytes(bytes.len() as u64)?;
        let original_hash = content_hash(&bytes);
        let original = decode_text_file(&canonical_path, bytes)?;
        let replacements = self.regex.find_iter(&original).count();
        if replacements == 0 {
            return Ok(());
        }
        self.record_replacements(replacements)?;
        self.files
            .push(self.build_file(canonical_path, original_hash, original, replacements)?);
        Ok(())
    }

    fn record_bytes(&mut self, bytes: u64) -> Result<()> {
        self.total_bytes = self
            .total_bytes
            .checked_add(bytes)
            .context("Total input size overflowed")?;
        if self.total_bytes > self.request.limits.max_total_bytes {
            bail!(
                "matched files exceed max_total_bytes limit of {}",
                self.request.limits.max_total_bytes
            );
        }
        Ok(())
    }

    fn record_replacements(&mut self, replacements: usize) -> Result<()> {
        self.total_replacements = self
            .total_replacements
            .checked_add(replacements)
            .context("Replacement count overflowed")?;
        if self.total_replacements > self.request.limits.max_matches {
            bail!(
                "matches exceed max_matches limit of {}",
                self.request.limits.max_matches
            );
        }
        Ok(())
    }

    fn build_file(
        &self,
        canonical_path: PathBuf,
        original_hash: String,
        original: String,
        replacements: usize,
    ) -> Result<PlannedFile> {
        let new_content = self
            .regex
            .replace_all(&original, self.replacement)
            .into_owned();
        let display_path = display_path(&self.request.cwd, &canonical_path);
        let permissions = fs::metadata(&canonical_path)
            .with_context(|| format!("Failed to read metadata for {}", canonical_path.display()))?
            .permissions();
        Ok(PlannedFile {
            diff: unified_diff(&display_path, &original, &new_content),
            line_changes: collect_line_changes(&original, self.regex, self.replacement),
            canonical_path,
            display_path,
            original_hash,
            original,
            replacement: new_content,
            permissions,
            replacements,
        })
    }

    fn finish(self, matched_files: usize) -> ReplacePlan {
        ReplacePlan {
            matched_files,
            total_replacements: self.total_replacements,
            hash: plan_hash(&self.files),
            files: self.files,
        }
    }
}

fn validate_expected_matches(expected: Option<usize>, actual: usize) -> Result<()> {
    if let Some(expected) = expected
        && expected != actual
    {
        bail!("expected {expected} matches, found {actual}");
    }
    Ok(())
}

fn validate_expected_plan_hash(expected: Option<&str>, actual: &str) -> Result<()> {
    if let Some(expected) = expected
        && expected != actual
    {
        bail!("replacement plan changed: expected {expected}, found {actual}");
    }
    Ok(())
}

fn decode_text_file(path: &Path, bytes: Vec<u8>) -> Result<String> {
    if bytes.contains(&0) {
        bail!("{} is binary or non-UTF-8", path.display());
    }
    String::from_utf8(bytes).with_context(|| format!("{} is binary or non-UTF-8", path.display()))
}

fn apply_plan(plan: &ReplacePlan) -> Result<()> {
    apply_plan_with(plan, |temporary, planned, _index| {
        persist_staged_file(temporary, planned)
    })
}

fn apply_plan_with<F>(plan: &ReplacePlan, mut persist: F) -> Result<()>
where
    F: FnMut(NamedTempFile, &PlannedFile, usize) -> Result<()>,
{
    let staged = stage_files(&plan.files)?;
    let mut committed = 0_usize;

    for (index, (temporary, planned)) in staged.into_iter().zip(&plan.files).enumerate() {
        if let Err(error) = persist(temporary, planned, index) {
            let rollback_error = rollback_files(&plan.files[..committed]).err();
            if let Some(rollback_error) = rollback_error {
                bail!(
                    "Failed to replace {}: {}; rollback also failed: {}",
                    planned.canonical_path.display(),
                    error,
                    rollback_error
                );
            }
            bail!(
                "Failed to replace {}: {}",
                planned.canonical_path.display(),
                error
            );
        }
        committed += 1;
    }
    Ok(())
}

fn persist_staged_file(temporary: NamedTempFile, planned: &PlannedFile) -> Result<()> {
    temporary
        .persist(&planned.canonical_path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to persist {}", planned.canonical_path.display()))?;
    Ok(())
}

fn stage_files(files: &[PlannedFile]) -> Result<Vec<NamedTempFile>> {
    files
        .iter()
        .map(|planned| stage_file(planned, &planned.replacement))
        .collect()
}

fn stage_file(planned: &PlannedFile, content: &str) -> Result<NamedTempFile> {
    let parent = planned.canonical_path.parent().with_context(|| {
        format!(
            "{} has no parent directory",
            planned.canonical_path.display()
        )
    })?;
    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to stage {}", planned.canonical_path.display()))?;
    temporary
        .write_all(content.as_bytes())
        .with_context(|| format!("Failed to stage {}", planned.canonical_path.display()))?;
    temporary
        .as_file()
        .set_permissions(planned.permissions.clone())
        .with_context(|| {
            format!(
                "Failed to preserve permissions for {}",
                planned.canonical_path.display()
            )
        })?;
    temporary.as_file().sync_all().with_context(|| {
        format!(
            "Failed to sync staged file for {}",
            planned.canonical_path.display()
        )
    })?;
    Ok(temporary)
}

fn rollback_files(files: &[PlannedFile]) -> Result<()> {
    for planned in files.iter().rev() {
        let temporary = stage_file(planned, &planned.original)?;
        persist_staged_file(temporary, planned)
            .with_context(|| format!("Failed to restore {}", planned.canonical_path.display()))?;
    }
    Ok(())
}

fn render_result(plan: ReplacePlan, dry_run: bool) -> ReplaceResult {
    let changes = plan
        .files
        .into_iter()
        .map(|planned| FileChange {
            path: planned.display_path,
            absolute_path: planned.canonical_path.to_string_lossy().to_string(),
            original_hash: planned.original_hash,
            replacements: planned.replacements,
            diff: planned.diff,
            line_changes: planned.line_changes,
        })
        .collect::<Vec<_>>();
    ReplaceResult {
        dry_run,
        plan_hash: plan.hash,
        matched_files: plan.matched_files,
        total_replacements: plan.total_replacements,
        files_modified: changes.len(),
        changes,
    }
}

fn request_paths(request: &ReplaceRequest) -> Result<Vec<PathBuf>> {
    if let Some(target_files) = &request.target_files {
        return canonical_target_files(target_files, request.limits.max_files);
    }
    collect_files(&request.cwd, &request.files, request.limits.max_files)
}

fn canonical_target_files(paths: &[PathBuf], max_files: usize) -> Result<Vec<PathBuf>> {
    let mut canonical_paths = BTreeSet::new();
    for path in paths {
        let canonical = fs::canonicalize(path)
            .with_context(|| format!("Failed to resolve frozen target {}", path.display()))?;
        if !canonical.is_file() {
            bail!("Frozen target is not a file: {}", canonical.display());
        }
        canonical_paths.insert(canonical);
        if canonical_paths.len() > max_files {
            bail!("matched files exceed max_files limit of {max_files}");
        }
    }
    Ok(canonical_paths.into_iter().collect())
}

fn collect_files(cwd: &Path, pattern: &str, max_files: usize) -> Result<Vec<PathBuf>> {
    let absolute_pattern = absolute_glob_pattern(cwd, pattern);
    let matcher = build_matcher(&absolute_pattern)?;
    let root = traversal_root(&absolute_pattern);
    if !root.exists() {
        return Ok(Vec::new());
    }
    collect_walked_files(&root, &matcher, max_files)
}

fn absolute_glob_pattern(cwd: &Path, pattern: &str) -> PathBuf {
    let pattern = pattern.strip_prefix('@').unwrap_or(pattern);
    if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        cwd.join(pattern)
    }
}

fn collect_walked_files(
    root: &Path,
    matcher: &GlobMatcher,
    max_files: usize,
) -> Result<Vec<PathBuf>> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .build();
    let mut canonical_paths = BTreeSet::new();
    for entry in walker {
        collect_walk_entry(entry, root, matcher, max_files, &mut canonical_paths)?;
    }
    Ok(canonical_paths.into_iter().collect())
}

fn collect_walk_entry(
    entry: std::result::Result<ignore::DirEntry, ignore::Error>,
    root: &Path,
    matcher: &GlobMatcher,
    max_files: usize,
    canonical_paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let entry = entry.with_context(|| format!("Failed to walk {}", root.display()))?;
    let is_file = entry
        .file_type()
        .is_some_and(|file_type| file_type.is_file());
    if !is_file || !matcher.is_match(entry.path()) {
        return Ok(());
    }
    let canonical = fs::canonicalize(entry.path())
        .with_context(|| format!("Failed to resolve {}", entry.path().display()))?;
    canonical_paths.insert(canonical);
    if canonical_paths.len() > max_files {
        bail!("matched files exceed max_files limit of {max_files}");
    }
    Ok(())
}

fn build_matcher(pattern: &Path) -> Result<GlobMatcher> {
    let pattern = pattern.to_string_lossy();
    GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .with_context(|| format!("Invalid glob pattern: {pattern}"))
        .map(|glob| glob.compile_matcher())
}

fn traversal_root(pattern: &Path) -> PathBuf {
    let mut root = PathBuf::new();
    for component in pattern.components() {
        if component_has_glob(&component) {
            break;
        }
        root.push(component.as_os_str());
    }
    if root == pattern {
        return root.parent().unwrap_or(&root).to_path_buf();
    }
    root
}

fn component_has_glob(component: &Component<'_>) -> bool {
    component
        .as_os_str()
        .to_string_lossy()
        .chars()
        .any(|character| matches!(character, '*' | '?' | '[' | '{'))
}

fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn unified_diff(path: &str, original: &str, replacement: &str) -> String {
    TextDiff::from_lines(original, replacement)
        .unified_diff()
        .header(path, path)
        .to_string()
}

fn collect_line_changes(original: &str, regex: &Regex, replacement: &str) -> Vec<LineChange> {
    original
        .lines()
        .enumerate()
        .filter_map(|(line_index, line)| {
            if !regex.is_match(line) {
                return None;
            }
            Some(LineChange {
                line_number: line_index + 1,
                before: line.to_string(),
                after: regex.replace_all(line, replacement).into_owned(),
            })
        })
        .collect()
}

fn content_hash(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn plan_hash(files: &[PlannedFile]) -> String {
    let mut hasher = Sha256::new();
    for file in files {
        hasher.update(file.display_path.as_bytes());
        hasher.update([0]);
        hasher.update(file.original_hash.as_bytes());
        hasher.update([0]);
        hasher.update(file.replacement.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn escape_non_numeric_dollars(value: &str) -> String {
    let value = unescape_sequences(value);
    let mut result = String::with_capacity(value.len() * 2);
    let chars = value.chars().collect::<Vec<_>>();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '$' {
            result.push(chars[index]);
            index += 1;
            continue;
        }
        index = escape_dollar_sequence(&chars, index, &mut result);
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
    let mut cursor = index + 1;
    result.push_str("${");
    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
        result.push(chars[cursor]);
        cursor += 1;
    }
    result.push('}');
    cursor
}

fn unescape_sequences(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
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
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_normalization_preserves_current_semantics() {
        assert_eq!(escape_non_numeric_dollars("$1_v2"), "${1}_v2");
        assert_eq!(escape_non_numeric_dollars("$request"), "$$request");
        assert_eq!(escape_non_numeric_dollars("$$foo"), "$$foo");
        assert_eq!(escape_non_numeric_dollars("$1\\n$2"), "${1}\n${2}");
    }

    #[test]
    fn failed_later_commit_rolls_back_earlier_files() {
        let directory = tempfile::TempDir::new().unwrap();
        let first_path = directory.path().join("a.txt");
        let second_path = directory.path().join("b.txt");
        fs::write(&first_path, "hello one\n").unwrap();
        fs::write(&second_path, "hello two\n").unwrap();
        let request = ReplaceRequest {
            cwd: directory.path().to_path_buf(),
            files: "**/*.txt".to_string(),
            pattern: "hello".to_string(),
            replacement: "goodbye".to_string(),
            dry_run: false,
            expected_matches: Some(2),
            expected_plan_hash: None,
            target_files: None,
            limits: ReplaceLimits::default(),
        };
        let regex = Regex::new(&request.pattern).unwrap();
        let replacement = escape_non_numeric_dollars(&request.replacement);
        let plan = plan_replacement(&request, &regex, &replacement).unwrap();

        let error = apply_plan_with(&plan, |temporary, planned, index| {
            if index == 1 {
                bail!("simulated second-file commit failure");
            }
            persist_staged_file(temporary, planned)
        })
        .unwrap_err()
        .to_string();

        assert!(error.contains("simulated second-file commit failure"));
        assert_eq!(fs::read_to_string(first_path).unwrap(), "hello one\n");
        assert_eq!(fs::read_to_string(second_path).unwrap(), "hello two\n");
    }
}
