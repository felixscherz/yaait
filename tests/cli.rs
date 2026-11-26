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
    assert_eq!(
        ids,
        vec![
            "claude-code",
            "codex",
            "deepseek",
            "github-copilot",
            "litellm",
            "openrouter"
        ]
    );
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
fn add_without_tracker_id_names_the_missing_argument_for_every_provider() {
    for provider in [
        "claude-code",
        "codex",
        "deepseek",
        "github-copilot",
        "litellm",
        "openrouter",
    ] {
        let output = yaait()
            .args(["--format", "json", "add", "--provider", provider])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{provider}");
        assert!(output.stderr.is_empty(), "{provider}");
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["command"], "cli", "{provider}");
        assert_eq!(response["errors"][0]["code"], "invalid_input", "{provider}");
        assert!(
            response["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("<TRACKER_ID>")
        );
    }
}

#[test]
fn human_add_error_identifies_the_missing_tracker_id() {
    let output = yaait()
        .args(["add", "--provider", "deepseek"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("<TRACKER_ID>"), "{stderr}");
}

#[test]
fn missing_add_provider_reports_the_required_argument() {
    let output = yaait().args(["add"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--provider <PROVIDER>"), "{stderr}");
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

#[cfg(target_os = "linux")]
#[test]
fn litellm_budget_exceeded_is_successful_in_human_and_json_usage() {
    use chrono::Utc;
    use httpmock::{Method::GET, MockServer};
    use yaait::{AppPaths, FileRegistry, Secret, TrackerManifest};

    let server = MockServer::start();
    for (token, spend, limit) in [("sk-exhausted", 41.5, 40.0), ("sk-active", 2.0, 10.0)] {
        server.mock(|when, then| {
            when.method(GET).path("/key/info")
                .header("authorization", format!("Bearer {token}"));
            if token == "sk-exhausted" {
                then.status(422).json_body(serde_json::json!({"error": {
                    "type": "budget_exceeded",
                    "message": format!("Budget has been exceeded! TeamMember={token} Current cost: {spend}, Max budget: {limit}")
                }}));
            } else {
                then.status(200).json_body(serde_json::json!({"info": {
                    "spend": spend, "max_budget": limit
                }}));
            }
        });
    }
    server.mock(|when, then| {
        when.method(GET)
            .path("/user/daily/activity/aggregated")
            .header("authorization", "Bearer sk-active");
        then.status(200)
            .json_body(serde_json::json!({"metadata": {"total_spend": 2.0}}));
    });

    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    let cache = home.path().join("cache");
    let registry = FileRegistry::new(AppPaths {
        data_root: data.join("yaait"),
        cache_root: cache.join("yaait"),
    });
    for (id, token) in [
        ("litellm-exhausted", "sk-exhausted"),
        ("litellm-active", "sk-active"),
    ] {
        let tracker = TrackerManifest {
            schema_version: 1,
            id: id.parse().unwrap(),
            provider: "litellm".parse().unwrap(),
            name: id.into(),
            description: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            settings: serde_json::json!({"base_url": server.base_url(), "window": "30d"})
                .as_object()
                .unwrap()
                .clone(),
            extensions: Default::default(),
        };
        registry
            .add(&tracker, &[("token".into(), Secret::new(token))].into())
            .unwrap();
    }
    for json in [false, true] {
        for details in [false, true] {
            let mut command = yaait();
            command
                .env("HOME", home.path())
                .env("XDG_DATA_HOME", &data)
                .env("XDG_CACHE_HOME", &cache);
            if json {
                command.args(["--format", "json"]);
            }
            command.args(["usage", "--refresh"]);
            if details {
                command.arg("--details");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(!stdout.contains("sk-exhausted"));
            assert!(!stdout.contains("sk-active"));
            if json {
                let response: Value = serde_json::from_str(&stdout).unwrap();
                assert_eq!(response["ok"], true);
                assert_eq!(response["partial"], false);
                assert_eq!(response["errors"], serde_json::json!([]));
                let trackers = response["data"]["trackers"].as_array().unwrap();
                assert_eq!(trackers.len(), 2);
                let exhausted = trackers
                    .iter()
                    .find(|t| t["id"] == "litellm-exhausted")
                    .unwrap();
                let budget = &exhausted["metrics"][0];
                assert_eq!(budget["remaining"], 0.0);
                assert_eq!(budget["used"], 41.5);
                assert_eq!(budget["limit"], 40.0);
                assert!(budget.get("resets_at").is_none());
            } else {
                assert!(stdout.contains("litellm-active:"));
                assert!(stdout.contains("litellm-exhausted:"));
                assert!(
                    stdout.contains("used: $41.50 of $40.00 (100.0%)"),
                    "{stdout}"
                );
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn litellm_without_a_budget_shows_spend_in_human_and_json_usage() {
    use chrono::Utc;
    use httpmock::{Method::GET, MockServer};
    use yaait::{AppPaths, FileRegistry, Secret, TrackerManifest};

    let server = MockServer::start();
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    let cache = home.path().join("cache");
    let registry = FileRegistry::new(AppPaths {
        data_root: data.join("yaait"),
        cache_root: cache.join("yaait"),
    });
    for (id, token, spend) in [
        ("litellm-personal", "sk-personal", 2.0),
        ("litellm-work", "sk-work", 3.5),
    ] {
        server.mock(|when, then| {
            when.method(GET)
                .path("/key/info")
                .header("authorization", format!("Bearer {token}"));
            then.status(200)
                .json_body(serde_json::json!({"info": {"max_budget": null}}));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/v2/user/info")
                .header("authorization", format!("Bearer {token}"));
            then.status(200)
                .json_body(serde_json::json!({"max_budget": null}));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/user/daily/activity/aggregated")
                .header("authorization", format!("Bearer {token}"));
            then.status(200).json_body(serde_json::json!({"metadata": {
                "total_spend": spend, "total_tokens": 100.0
            }}));
        });
        let tracker = TrackerManifest {
            schema_version: 1,
            id: id.parse().unwrap(),
            provider: "litellm".parse().unwrap(),
            name: id.into(),
            description: None,
            enabled: true,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            settings: serde_json::json!({"base_url": server.base_url(), "window": "30d"})
                .as_object()
                .unwrap()
                .clone(),
            extensions: Default::default(),
        };
        registry
            .add(&tracker, &[("token".into(), Secret::new(token))].into())
            .unwrap();
    }
    for json in [false, true] {
        for details in [false, true] {
            let mut command = yaait();
            command
                .env("HOME", home.path())
                .env("XDG_DATA_HOME", &data)
                .env("XDG_CACHE_HOME", &cache);
            if json {
                command.args(["--format", "json"]);
            }
            command.args(["usage", "--refresh"]);
            if details {
                command.arg("--details");
            }
            let output = command.output().unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(!stdout.contains("sk-personal"));
            assert!(!stdout.contains("sk-work"));
            if json {
                let response: Value = serde_json::from_str(&stdout).unwrap();
                assert_eq!(response["schema_version"], 2);
                assert_eq!(response["ok"], true);
                let trackers = response["data"]["trackers"].as_array().unwrap();
                assert_eq!(trackers.len(), 2);
                for (id, value) in [("litellm-personal", 2.0), ("litellm-work", 3.5)] {
                    let tracker = trackers.iter().find(|t| t["id"] == id).unwrap();
                    let metrics = tracker["metrics"].as_array().unwrap();
                    assert_eq!(metrics.len(), if details { 2 } else { 1 });
                    let spend = &metrics[0];
                    assert_eq!(spend["id"], "spend");
                    assert_eq!(spend["tier"], "primary");
                    assert_eq!(spend["value"], value);
                    assert_eq!(spend["period"], "30d");
                    assert!(spend.get("limit").is_none());
                    assert!(spend.get("remaining").is_none());
                }
            } else {
                assert!(
                    stdout.contains("litellm-personal:\n    Spend: $2.00"),
                    "{stdout}"
                );
                assert!(
                    stdout.contains("litellm-work:\n    Spend: $3.50"),
                    "{stdout}"
                );
                assert_eq!(stdout.contains("Total tokens:"), details);
            }
        }
    }
}
