use regex_replace_mcp::{ReplaceLimits, ReplaceRequest, replace};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn write_file(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn request(root: &Path, expected_matches: usize) -> ReplaceRequest {
    ReplaceRequest {
        cwd: root.to_path_buf(),
        files: "**/*.txt".to_string(),
        pattern: "hello".to_string(),
        replacement: "goodbye".to_string(),
        dry_run: true,
        expected_matches: Some(expected_matches),
        expected_plan_hash: None,
        target_files: None,
        limits: ReplaceLimits::default(),
    }
}

#[test]
fn expected_match_mismatch_prevents_all_writes() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello one");
    write_file(dir.path(), "b.txt", b"hello two");

    let mut replace_request = request(dir.path(), 1);
    replace_request.dry_run = false;

    let error = replace(replace_request).unwrap_err().to_string();

    assert!(error.contains("expected 1 matches, found 2"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello one"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "hello two"
    );
}

#[test]
fn dry_run_returns_stable_unified_diffs_without_writing() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello world\n");

    let first = replace(request(dir.path(), 1)).unwrap();
    let second = replace(request(dir.path(), 1)).unwrap();

    assert_eq!(first.plan_hash, second.plan_hash);
    assert_eq!(first.total_replacements, 1);
    assert_eq!(first.files_modified, 1);
    assert_eq!(first.changes.len(), 1);
    assert!(first.changes[0].diff.contains("--- a.txt"));
    assert!(first.changes[0].diff.contains("+++ a.txt"));
    assert!(first.changes[0].diff.contains("-hello world"));
    assert!(first.changes[0].diff.contains("+goodbye world"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello world\n"
    );
}

#[test]
fn apply_requires_the_current_plan_hash() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello world\n");

    let plan = replace(request(dir.path(), 1)).unwrap();
    write_file(dir.path(), "a.txt", b"hello changed\n");

    let mut apply_request = request(dir.path(), 1);
    apply_request.dry_run = false;
    apply_request.expected_plan_hash = Some(plan.plan_hash);

    let error = replace(apply_request).unwrap_err().to_string();

    assert!(error.contains("replacement plan changed"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello changed\n"
    );
}

#[test]
fn apply_updates_all_planned_files() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello one\n");
    write_file(dir.path(), "b.txt", b"hello two\n");

    let plan = replace(request(dir.path(), 2)).unwrap();
    let mut apply_request = request(dir.path(), 2);
    apply_request.dry_run = false;
    apply_request.expected_plan_hash = Some(plan.plan_hash.clone());

    let result = replace(apply_request).unwrap();

    assert_eq!(result.plan_hash, plan.plan_hash);
    assert_eq!(result.total_replacements, 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "goodbye one\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "goodbye two\n"
    );
}

#[test]
fn gitignored_files_are_not_selected() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    write_file(dir.path(), ".gitignore", b"ignored.txt\n");
    write_file(dir.path(), "included.txt", b"hello included\n");
    write_file(dir.path(), "ignored.txt", b"hello ignored\n");

    let result = replace(request(dir.path(), 1)).unwrap();

    assert_eq!(result.changes.len(), 1);
    assert!(result.changes[0].path.ends_with("included.txt"));
}

#[test]
fn binary_input_rejection_prevents_text_file_writes() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello text\n");
    write_file(dir.path(), "binary.txt", b"hello\0binary");

    let mut replace_request = request(dir.path(), 2);
    replace_request.dry_run = false;

    let error = replace(replace_request).unwrap_err().to_string();

    assert!(error.contains("binary or non-UTF-8"));
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello text\n"
    );
}

#[test]
fn configured_limits_fail_before_writing() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.txt", b"hello one hello two\n");
    write_file(dir.path(), "b.txt", b"hello three\n");

    let cases = [
        ReplaceLimits {
            max_files: 1,
            ..ReplaceLimits::default()
        },
        ReplaceLimits {
            max_total_bytes: 1,
            ..ReplaceLimits::default()
        },
        ReplaceLimits {
            max_matches: 2,
            ..ReplaceLimits::default()
        },
    ];

    for limits in cases {
        let mut replace_request = request(dir.path(), 3);
        replace_request.dry_run = false;
        replace_request.limits = limits;

        assert!(replace(replace_request).is_err());
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "hello one hello two\n"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "hello three\n"
        );
    }
}
