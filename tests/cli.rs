use std::process::Command;

use directories::ProjectDirs;
use serde_json::Value;

fn yaait() -> Command {
    Command::new(env!("CARGO_BIN_EXE_yaait"))
}

#[test]
fn provider_discovery_uses_the_versioned_json_envelope() {
    let output = yaait()
        .args(["--format", "json", "providers", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 2);
    assert_eq!(response["command"], "providers.list");
    assert_eq!(response["ok"], true);
    let ids: Vec<_> = response["data"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|provider| provider["id"].as_str().unwrap())
        .collect();
    for expected in [
        "github-copilot",
        "litellm",
        "deepseek",
        "openrouter",
        "codex",
    ] {
        assert!(ids.contains(&expected));
    }
}

#[test]
fn invalid_cli_arguments_are_json_and_fail() {
    let output = yaait()
        .args(["--format", "json", "not-a-command"])
        .output()
        .unwrap();
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

#[test]
fn no_command_shows_the_onboarding_help_on_stdout() {
    let implicit = yaait().output().unwrap();
    let explicit = yaait().arg("--help").output().unwrap();
    assert!(implicit.status.success());
    assert!(implicit.stderr.is_empty());
    assert_eq!(implicit.stdout, explicit.stdout);
    let help = String::from_utf8(implicit.stdout).unwrap();
    assert!(help.contains("yaait providers list"));
    assert!(help.contains("yaait providers describe github-copilot"));
    assert!(help.contains("yaait add --provider github-copilot copilot-personal"));
    assert!(help.contains("yaait --format json usage --details"));
}

#[test]
fn onboarding_subcommands_explain_provider_discovery_and_setup_input() {
    for (args, expected) in [
        (vec!["providers", "--help"], "required setup fields"),
        (vec!["add", "--help"], "yaait providers list"),
        (vec!["setup", "--help"], "JSON object from stdin"),
    ] {
        let output = yaait().args(args).output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(String::from_utf8(output.stdout).unwrap().contains(expected));
    }
}

#[test]
fn human_format_renders_provider_list_as_text() {
    let output = yaait()
        .args(["--format", "human", "providers", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("github-copilot:"));
    assert!(stdout.contains("litellm:"));
    assert!(serde_json::from_str::<Value>(&stdout).is_err());
}

#[test]
fn human_output_is_the_default() {
    let output = yaait().args(["providers", "list"]).output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("github-copilot:"));
    assert!(serde_json::from_str::<Value>(&stdout).is_err());
}

#[test]
fn human_shortcut_renders_provider_list_as_text() {
    let output = yaait()
        .args(["--human", "providers", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("github-copilot:"));
    assert!(stdout.contains("litellm:"));
    assert!(serde_json::from_str::<Value>(&stdout).is_err());
}

#[test]
fn human_format_renders_debug_as_text() {
    let output = yaait()
        .args(["debug", "--format", "human"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("app_dir:"));
    assert!(stdout.contains("cache_dir:"));
}

#[test]
fn human_shortcut_reports_invalid_arguments_on_stderr() {
    let output = yaait().args(["--human", "not-a-command"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error: [invalid_input]"));
}

#[test]
fn human_format_reports_invalid_arguments_on_stderr() {
    let output = yaait()
        .args(["--format", "human", "not-a-command"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error: [invalid_input]"));
}

#[test]
fn usage_primary_view_is_available() {
    let home = tempfile::tempdir().unwrap();
    let output = yaait()
        .env("HOME", home.path())
        .args(["usage", "--primary"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "no trackers configured\n"
    );
}

#[test]
fn usage_defaults_to_the_primary_human_view() {
    let home = tempfile::tempdir().unwrap();
    let output = yaait()
        .env("HOME", home.path())
        .arg("usage")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "no trackers configured\n"
    );
}

#[test]
fn usage_details_view_is_available() {
    let home = tempfile::tempdir().unwrap();
    let output = yaait()
        .env("HOME", home.path())
        .args(["usage", "--details"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "no trackers configured\n"
    );
}

#[test]
fn usage_metric_views_are_mutually_exclusive() {
    let output = yaait()
        .args(["usage", "--primary", "--details"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cannot be used with"));
}

#[test]
fn debug_shows_the_effective_application_directories() {
    let output = yaait()
        .args(["--format", "json", "debug"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected = ProjectDirs::from("", "", "yaait").unwrap();
    assert_eq!(response["command"], "debug");
    assert_eq!(response["ok"], true);
    assert_eq!(
        response["data"]["app_dir"],
        expected.data_dir().to_string_lossy().as_ref()
    );
    assert_eq!(
        response["data"]["cache_dir"],
        expected.cache_dir().to_string_lossy().as_ref()
    );
}

#[test]
fn update_rejects_invalid_versions_with_a_json_failure() {
    let output = yaait()
        .args(["--format", "json", "update", "--version", "bad"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 2);
    assert_eq!(response["command"], "update");
    assert_eq!(response["errors"][0]["code"], "invalid_input");
}

#[test]
fn unmanaged_update_reports_original_installation_instructions() {
    let temp = tempfile::tempdir().unwrap();
    for json in [false, true] {
        let mut command = yaait();
        if json {
            command.args(["--format", "json"]);
        }
        let output = command
            .arg("update")
            .env("AXOUPDATER_CONFIG_PATH", temp.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        if json {
            assert!(output.stderr.is_empty());
            let response: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(response["errors"][0]["code"], "update_unsupported");
            assert_eq!(
                response["errors"][0]["details"]["installation_method"],
                "unmanaged"
            );
        } else {
            assert!(output.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("original installation method")
            );
        }
    }
}

#[test]
fn deepseek_setup_is_discoverable() {
    let output = yaait()
        .args(["--format", "json", "providers", "describe", "deepseek"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response.to_string().contains("token"));
    assert!(response.to_string().contains("secret"));
    let output = yaait().args(["providers", "list"]).output().unwrap();
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("deepseek:")
    );
}

#[test]
fn openrouter_setup_is_discoverable() {
    let output = yaait()
        .args(["--format", "json", "providers", "describe", "openrouter"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response.to_string().contains("token"));
    assert!(response.to_string().contains("secret"));
    let output = yaait().args(["providers", "list"]).output().unwrap();
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("openrouter:")
    );
}
