use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_weavie"));
    command.current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"));
    command
}

#[test]
fn graph_json_selects_only_app_dependencies() {
    let output = command()
        .args(["graph", "--from", "app", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let graph: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(graph["root"], "app");
    assert_eq!(graph["nodes"].as_object().unwrap().len(), 3);
    assert!(graph["nodes"].get("tool").is_none());
}

#[test]
fn why_prints_the_dependency_chain_and_errors_for_unreachable_inputs() {
    let output = command()
        .args(["why", "protocol", "--from", "app"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("app --transport--> transport"));
    assert!(stdout.contains("transport --protocol--> protocol"));
    let output = command()
        .args(["why", "protocol", "--from", "tool"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no declared input path"));
}

#[test]
fn reads_metadata_from_stdin() {
    let lock: Value = serde_json::from_str(include_str!("fixtures/flake.lock")).unwrap();
    let metadata = serde_json::json!({"locks": lock}).to_string();
    let mut child = command()
        .args(["--file", "-", "why", "protocol", "--from", "app", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(metadata.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let explanation: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(explanation["steps"].as_array().unwrap().len(), 2);
}

#[test]
fn missing_files_and_invalid_commands_fail_clearly() {
    let output = command()
        .args(["graph", "--file", "absent.lock"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("reading absent.lock"));
    let output = command().arg("not-a-command").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"));
}
