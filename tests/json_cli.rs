use serde_json::{Value, json};
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn run_cli(dir: &TempDir, request: &Value) -> std::process::Output {
    let request_path = dir.path().join("request.json");
    fs::write(&request_path, request.to_string()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_regex-replace-json"))
        .arg(request_path)
        .output()
        .unwrap()
}

fn base_request(dir: &TempDir, action: &str) -> Value {
    json!({
        "action": action,
        "cwd": dir.path(),
        "files": "**/*.txt",
        "pattern": "hello",
        "replacement": "goodbye",
        "expectedMatches": 1,
        "maxFiles": 10,
        "maxTotalBytes": 1024,
        "maxMatches": 10
    })
}

#[test]
fn json_cli_plans_and_applies_the_approved_replacement() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.txt");
    fs::write(&path, "hello world\n").unwrap();

    let plan_output = run_cli(&dir, &base_request(&dir, "plan"));
    assert!(plan_output.status.success());
    let plan: Value = serde_json::from_slice(&plan_output.stdout).unwrap();
    assert_eq!(plan["totalReplacements"], 1);
    assert_eq!(plan["changes"][0]["path"], "test.txt");
    assert!(
        plan["changes"][0]["diff"]
            .as_str()
            .unwrap()
            .contains("+goodbye world")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello world\n");

    let unplanned_path = dir.path().join("unplanned.txt");
    fs::write(&unplanned_path, "hello unplanned\n").unwrap();
    let mut apply_request = base_request(&dir, "apply");
    apply_request["planHash"] = plan["planHash"].clone();
    apply_request["targets"] = Value::Array(
        plan["changes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|change| change["absolutePath"].clone())
            .collect(),
    );
    let apply_output = run_cli(&dir, &apply_request);
    assert!(apply_output.status.success());
    let applied: Value = serde_json::from_slice(&apply_output.stdout).unwrap();
    assert_eq!(applied["dryRun"], false);
    assert_eq!(fs::read_to_string(path).unwrap(), "goodbye world\n");
    assert_eq!(
        fs::read_to_string(unplanned_path).unwrap(),
        "hello unplanned\n"
    );
}

#[test]
fn json_cli_rejects_zero_expected_matches() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.txt"), "nothing here\n").unwrap();
    let mut request = base_request(&dir, "plan");
    request["expectedMatches"] = json!(0);

    let output = run_cli(&dir, &request);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("expectedMatches must be greater than zero")
    );
}

#[test]
fn json_cli_reports_validation_failures_on_stderr() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.txt"), "hello world\n").unwrap();
    let mut bad_request = base_request(&dir, "plan");
    bad_request["expectedMatches"] = json!(2);

    let output = run_cli(&dir, &bad_request);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("expected 2 matches, found 1")
    );
}
