use std::process::Command;

use serde_json::Value;

fn yaait() -> Command {
    Command::new(env!("CARGO_BIN_EXE_yaait"))
}

#[test]
fn provider_discovery_uses_the_versioned_json_envelope() {
    let output = yaait().args(["providers", "list"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 1);
    assert_eq!(response["command"], "providers.list");
    assert_eq!(response["ok"], true);
    assert_eq!(response["data"]["providers"][0]["id"], "github-copilot");
}

#[test]
fn invalid_cli_arguments_are_json_and_fail() {
    let output = yaait().arg("not-a-command").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["command"], "cli");
    assert_eq!(response["ok"], false);
    assert_eq!(response["errors"][0]["code"], "invalid_input");
}

#[test]
fn help_remains_plain_text() {
    let output = yaait().arg("--help").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("Usage:"));
}
