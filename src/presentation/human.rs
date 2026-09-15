use serde_json::Value;

use super::Envelope;
use crate::TrackerError;

pub struct Rendered {
    pub stdout: String,
    pub stderr: String,
}

pub fn render(envelope: &Envelope) -> Rendered {
    let stdout = match envelope.command.as_str() {
        "usage" => {
            let rendered = render_usage(&envelope.data);
            if rendered.is_empty() && envelope.errors.is_empty() {
                "no trackers configured\n".into()
            } else {
                rendered
            }
        }
        "list" => render_tracker_list(&envelope.data),
        "show" => render_tracker_detail(&envelope.data),
        "add" => confirmation("added tracker", &envelope.data),
        "setup" => confirmation("updated tracker", &envelope.data),
        "enable" => confirmation("enabled tracker", &envelope.data),
        "disable" => confirmation("disabled tracker", &envelope.data),
        "remove" => render_remove(&envelope.data),
        "providers.list" => render_provider_list(&envelope.data),
        "providers.describe" => render_provider_detail(&envelope.data),
        "debug" => render_debug(&envelope.data),
        _ => String::new(),
    };
    let mut stderr = String::new();
    for warning in &envelope.warnings {
        push_diagnostic(&mut stderr, "warning", warning);
    }
    for error in &envelope.errors {
        push_diagnostic(&mut stderr, "error", error);
    }
    Rendered {
        stdout: stdout.trim_end().to_owned(),
        stderr: stderr.trim_end().to_owned(),
    }
}

fn render_usage(data: &Value) -> String {
    let mut out = String::new();
    let Some(trackers) = data.get("trackers").and_then(Value::as_array) else {
        return out;
    };
    for tracker in trackers {
        push_line(
            &mut out,
            0,
            &format!("{}:", text(tracker, "id").unwrap_or("unknown")),
        );
        let Some(metrics) = tracker.get("metrics").and_then(Value::as_array) else {
            continue;
        };
        let mut sorted: Vec<&Value> = metrics.iter().collect();
        sorted.sort_by_key(|metric| metric.get("tier").and_then(Value::as_str) != Some("primary"));
        let single = sorted.len() == 1;
        let observed_at = text(tracker, "observed_at");
        for metric in sorted {
            let lines = metric_lines(metric, observed_at);
            if single || metric.get("value").is_some() {
                for line in lines {
                    push_line(&mut out, 4, &line);
                }
            } else {
                let label = text(metric, "label").unwrap_or("metric");
                push_line(&mut out, 4, &format!("{label}:"));
                for line in lines {
                    push_line(&mut out, 8, &line);
                }
            }
        }
    }
    out
}

fn metric_lines(metric: &Value, observed_at: Option<&str>) -> Vec<String> {
    let unit = text(metric, "unit").unwrap_or("");
    if let Some(value) = number(metric, "value") {
        let label = text(metric, "label").unwrap_or("value");
        return vec![format!("{label}: {}", amount_with_unit(value, unit))];
    }
    let mut lines = Vec::new();
    let limit = number(metric, "limit");
    let has_used = metric.get("used").is_some_and(Value::is_number);
    if let Some(remaining) = number(metric, "remaining") {
        let percent = metric
            .get("attributes")
            .and_then(|attributes| attributes.get("percent_remaining"))
            .and_then(Value::as_f64)
            .or_else(|| {
                if has_used {
                    None
                } else {
                    percent_of(remaining, limit)
                }
            });
        let mut line = format!("remaining: {}", fmt_amount(remaining, unit));
        if let Some(limit) = limit {
            line.push_str(&format!(" of {}", fmt_amount(limit, unit)));
        }
        line.push_str(&unit_suffix(unit, limit.unwrap_or(remaining)));
        if let Some(percent) = percent {
            line.push_str(&format!(" ({percent:.1}%)"));
        }
        lines.push(line);
    }
    if let Some(used) = number(metric, "used") {
        let mut line = format!("used: {}", fmt_amount(used, unit));
        line.push_str(&unit_suffix(unit, used));
        if let Some(percent) = percent_of(used, limit) {
            line.push_str(&format!(" ({percent:.1}%)"));
        }
        lines.push(line);
    }
    if lines.is_empty()
        && let Some(limit) = limit
    {
        lines.push(format!(
            "limit: {}{}",
            fmt_amount(limit, unit),
            unit_suffix(unit, limit)
        ));
    }
    if let Some(resets_at) = text(metric, "resets_at") {
        let mut line = format!("resets_at: {}", fmt_timestamp(resets_at));
        if let Some(observed_at) = observed_at
            && let Some(relative) = relative_timestamp(resets_at, observed_at)
        {
            line.push_str(&format!(" ({relative})"));
        }
        lines.push(line);
    }
    lines
}

fn render_tracker_list(data: &Value) -> String {
    let mut out = String::new();
    let Some(trackers) = data.get("trackers").and_then(Value::as_array) else {
        return out;
    };
    if trackers.is_empty() {
        return "no trackers configured\n".into();
    }
    for tracker in trackers {
        let id = text(tracker, "id").unwrap_or("unknown");
        let provider = text(tracker, "provider").unwrap_or("unknown");
        let status = if tracker
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "enabled"
        } else {
            "disabled"
        };
        push_line(&mut out, 0, &format!("{id} ({provider}) {status}"));
    }
    out
}

fn render_tracker_detail(data: &Value) -> String {
    let mut out = String::new();
    let Some(tracker) = data.get("tracker") else {
        return out;
    };
    for key in [
        "id",
        "provider",
        "name",
        "description",
        "enabled",
        "created_at",
        "updated_at",
    ] {
        match tracker.get(key) {
            Some(Value::String(value)) => push_line(&mut out, 0, &format!("{key}: {value}")),
            Some(Value::Bool(value)) => push_line(&mut out, 0, &format!("{key}: {value}")),
            _ => {}
        }
    }
    if let Some(settings) = tracker.get("settings").and_then(Value::as_object) {
        for (key, value) in settings {
            push_line(&mut out, 0, &format!("{key}: {}", scalar(value)));
        }
    }
    out
}

fn confirmation(verb: &str, data: &Value) -> String {
    match data.get("tracker").and_then(|tracker| text(tracker, "id")) {
        Some(id) => format!("{verb} {id}\n"),
        None => String::new(),
    }
}

fn render_remove(data: &Value) -> String {
    match data.get("removed") {
        Some(Value::Null) | None => "nothing removed\n".into(),
        Some(removed) => format!(
            "removed tracker {}\n",
            text(removed, "id").unwrap_or("unknown")
        ),
    }
}

fn render_provider_list(data: &Value) -> String {
    let mut out = String::new();
    let Some(providers) = data.get("providers").and_then(Value::as_array) else {
        return out;
    };
    for provider in providers {
        let id = text(provider, "id").unwrap_or("unknown");
        let name = text(provider, "name").unwrap_or("");
        let description = text(provider, "description").unwrap_or("");
        push_line(&mut out, 0, &format!("{id}: {name} - {description}"));
    }
    out
}

fn render_provider_detail(data: &Value) -> String {
    let mut out = String::new();
    let Some(provider) = data.get("provider") else {
        return out;
    };
    let id = text(provider, "id").unwrap_or("unknown");
    let name = text(provider, "name").unwrap_or("");
    push_line(&mut out, 0, &format!("{id}: {name}"));
    if let Some(description) = text(provider, "description") {
        push_line(&mut out, 0, description);
    }
    if let Some(fields) = provider
        .get("setup")
        .and_then(|setup| setup.get("fields"))
        .and_then(Value::as_array)
        .filter(|fields| !fields.is_empty())
    {
        push_line(&mut out, 0, "setup fields:");
        for field in fields {
            let label = text(field, "label").unwrap_or("field");
            let key = text(field, "key").unwrap_or("unknown");
            let kind = text(field, "kind").unwrap_or("string");
            let required = if field
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                ", required"
            } else {
                ""
            };
            push_line(&mut out, 4, &format!("{label} ({key}: {kind}{required})"));
        }
    }
    if let Some(metrics) = provider
        .get("metrics")
        .and_then(Value::as_array)
        .filter(|metrics| !metrics.is_empty())
    {
        push_line(&mut out, 0, "metrics:");
        for metric in metrics {
            let label = text(metric, "label").unwrap_or("metric");
            let kind = text(metric, "kind").unwrap_or("unknown");
            let unit = text(metric, "unit").unwrap_or("");
            push_line(&mut out, 4, &format!("{label} ({kind}, {unit})"));
        }
    }
    out
}

fn render_debug(data: &Value) -> String {
    let mut out = String::new();
    for key in ["app_dir", "cache_dir"] {
        if let Some(value) = text(data, key) {
            push_line(&mut out, 0, &format!("{key}: {value}"));
        }
    }
    out
}

fn push_diagnostic(out: &mut String, level: &str, error: &TrackerError) {
    let mut line = format!("{level}: [{}] {}", error.code, error.message);
    let mut context = Vec::new();
    if let Some(tracker) = &error.tracker_id {
        context.push(format!("tracker: {tracker}"));
    }
    if let Some(provider) = &error.provider {
        context.push(format!("provider: {provider}"));
    }
    if !context.is_empty() {
        line.push_str(&format!(" ({})", context.join(", ")));
    }
    out.push_str(&line);
    out.push('\n');
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn push_line(out: &mut String, indent: usize, line: &str) {
    out.push_str(&" ".repeat(indent));
    out.push_str(line);
    out.push('\n');
}

fn percent_of(amount: f64, limit: Option<f64>) -> Option<f64> {
    limit.filter(|limit| *limit > 0.0).map(|limit| {
        let percent = amount / limit * 100.0;
        (percent * 10.0).round() / 10.0
    })
}

fn amount_with_unit(value: f64, unit: &str) -> String {
    format!("{}{}", fmt_amount(value, unit), unit_suffix(unit, value))
}

fn fmt_amount(value: f64, unit: &str) -> String {
    if unit == "usd" {
        format!("${value:.2}")
    } else {
        fmt_number(value)
    }
}

fn fmt_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        group_thousands(value as i64)
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

fn group_thousands(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{sign}{grouped}")
}

fn unit_suffix(unit: &str, basis: f64) -> String {
    match unit {
        "" | "usd" => String::new(),
        unit if basis == 1.0 || unit.ends_with('s') => format!(" {unit}"),
        unit => format!(" {unit}s"),
    }
}

fn fmt_timestamp(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_else(|_| raw.to_owned())
}

fn relative_timestamp(resets_at: &str, observed_at: &str) -> Option<String> {
    let resets = chrono::DateTime::parse_from_rfc3339(resets_at).ok()?;
    let observed = chrono::DateTime::parse_from_rfc3339(observed_at).ok()?;
    Some(describe_duration(resets.signed_duration_since(observed)))
}

fn describe_duration(duration: chrono::Duration) -> String {
    if duration < chrono::Duration::zero() {
        return "in the past".into();
    }
    let seconds = duration.num_seconds();
    if seconds < 60 {
        return "in under a minute".into();
    }
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(time_unit(days, "day"));
    }
    if hours > 0 {
        parts.push(time_unit(hours, "hour"));
    }
    if parts.len() < 2 && minutes > 0 {
        parts.push(time_unit(minutes, "minute"));
    }
    parts.truncate(2);
    format!("in {}", parts.join(", "))
}

fn time_unit(value: i64, label: &str) -> String {
    let suffix = if value == 1 { "" } else { "s" };
    format!("{value} {label}{suffix}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn envelope(command: &str, data: Value) -> Envelope {
        Envelope {
            schema_version: 2,
            command: command.into(),
            ok: true,
            partial: false,
            data,
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn usage_renders_request_quota_on_indented_lines() {
        let data = json!({"trackers": [{
            "id": "copilot-rct",
            "provider": "github-copilot",
            "metrics": [{
                "id": "premium-interactions",
                "label": "Premium interactions",
                "kind": "quota",
                "unit": "request",
                "remaining": 4353.0,
                "limit": 10000.0,
                "resets_at": "2026-10-01T00:00:00Z",
                "attributes": {"percent_remaining": 43.5}
            }]
        }]});
        let rendered = render(&envelope("usage", data));
        assert_eq!(
            rendered.stdout,
            "copilot-rct:\n    remaining: 4,353 of 10,000 requests (43.5%)\n    resets_at: 2026-10-01 00:00:00Z"
        );
        assert!(rendered.stderr.is_empty());
    }

    #[test]
    fn usage_renders_usd_budget_with_used_share() {
        let data = json!({"trackers": [{
            "id": "litellm-exxeta",
            "provider": "litellm",
            "metrics": [{
                "id": "budget",
                "label": "User budget",
                "kind": "quota",
                "unit": "usd",
                "used": 7.604783539999995,
                "remaining": 32.39521646000001,
                "limit": 40.0,
                "resets_at": "2026-10-01T00:00:00Z",
                "attributes": {"scope": "user"}
            }]
        }]});
        let rendered = render(&envelope("usage", data));
        assert_eq!(
            rendered.stdout,
            "litellm-exxeta:\n    remaining: $32.40 of $40.00\n    used: $7.60 (19.0%)\n    resets_at: 2026-10-01 00:00:00Z"
        );
    }

    #[test]
    fn usage_groups_quota_metrics_under_their_label() {
        let data = json!({"trackers": [{
            "id": "litellm-exxeta",
            "provider": "litellm",
            "metrics": [
                {
                    "id": "budget",
                    "label": "User budget",
                    "kind": "quota",
                    "unit": "usd",
                    "remaining": 32.4,
                    "limit": 40.0
                },
                {
                    "id": "total-tokens",
                    "label": "Total tokens",
                    "kind": "counter",
                    "unit": "token",
                    "value": 152334.0
                }
            ]
        }]});
        let rendered = render(&envelope("usage", data));
        assert_eq!(
            rendered.stdout,
            "litellm-exxeta:\n    User budget:\n        remaining: $32.40 of $40.00 (81.0%)\n    Total tokens: 152,334 tokens"
        );
    }

    #[test]
    fn usage_shows_time_until_reset_relative_to_observation() {
        let data = json!({"trackers": [{
            "id": "copilot-rct",
            "provider": "github-copilot",
            "observed_at": "2026-09-15T09:19:44Z",
            "metrics": [{
                "id": "premium-interactions",
                "label": "Premium interactions",
                "kind": "quota",
                "unit": "request",
                "remaining": 4353.0,
                "limit": 10000.0,
                "resets_at": "2026-10-01T00:00:00Z",
                "attributes": {"percent_remaining": 43.5}
            }]
        }]});
        let rendered = render(&envelope("usage", data));
        assert_eq!(
            rendered.stdout,
            "copilot-rct:\n    remaining: 4,353 of 10,000 requests (43.5%)\n    resets_at: 2026-10-01 00:00:00Z (in 15 days, 14 hours)"
        );
    }

    #[test]
    fn durations_use_the_two_largest_units() {
        assert_eq!(
            describe_duration(chrono::Duration::seconds(45)),
            "in under a minute"
        );
        assert_eq!(
            describe_duration(chrono::Duration::minutes(45)),
            "in 45 minutes"
        );
        assert_eq!(
            describe_duration(chrono::Duration::hours(14) + chrono::Duration::minutes(32)),
            "in 14 hours, 32 minutes"
        );
        assert_eq!(describe_duration(chrono::Duration::days(1)), "in 1 day");
        assert_eq!(describe_duration(chrono::Duration::days(-2)), "in the past");
    }

    #[test]
    fn usage_explains_when_no_trackers_exist() {
        let rendered = render(&envelope("usage", json!({"trackers": []})));
        assert_eq!(rendered.stdout, "no trackers configured");
    }

    #[test]
    fn usage_reports_errors_on_stderr() {
        let mut envelope = envelope("usage", json!({"trackers": []}));
        envelope.ok = false;
        envelope.errors.push(
            TrackerError::new("authentication_failed", "token rejected")
                .tracker("work")
                .provider("fake"),
        );
        let rendered = render(&envelope);
        assert!(rendered.stdout.is_empty());
        assert_eq!(
            rendered.stderr,
            "error: [authentication_failed] token rejected (tracker: work, provider: fake)"
        );
    }

    #[test]
    fn list_renders_tracker_status_lines() {
        let data = json!({"trackers": [
            {"id": "work", "provider": "fake", "enabled": true},
            {"id": "old", "provider": "fake", "enabled": false}
        ]});
        let rendered = render(&envelope("list", data));
        assert_eq!(rendered.stdout, "work (fake) enabled\nold (fake) disabled");
    }

    #[test]
    fn list_explains_when_no_trackers_exist() {
        let rendered = render(&envelope("list", json!({"trackers": []})));
        assert_eq!(rendered.stdout, "no trackers configured");
    }

    #[test]
    fn mutations_confirm_the_tracker_id() {
        let data = json!({"tracker": {"id": "work"}});
        assert_eq!(
            render(&envelope("add", data.clone())).stdout,
            "added tracker work"
        );
        assert_eq!(
            render(&envelope("disable", data)).stdout,
            "disabled tracker work"
        );
    }

    #[test]
    fn remove_reports_the_outcome() {
        assert_eq!(
            render(&envelope("remove", json!({"removed": {"id": "work"}}))).stdout,
            "removed tracker work"
        );
        assert_eq!(
            render(&envelope("remove", json!({"removed": null}))).stdout,
            "nothing removed"
        );
    }

    #[test]
    fn numbers_are_grouped_and_trimmed() {
        assert_eq!(fmt_number(152334.0), "152,334");
        assert_eq!(fmt_number(32.4), "32.4");
        assert_eq!(fmt_number(0.5), "0.5");
    }
}
