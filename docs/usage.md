# Usage

List providers, add a named tracker, and check usage:

```sh
yaait providers list
yaait add --provider github-copilot copilot-personal
yaait usage
```

Setup prompts for credentials. Add another subscription with a different
tracker ID, even if it uses the same provider:

```sh
yaait add --provider github-copilot copilot-work
```

See the [provider guides](index.md#supported-providers) for setup details.
`yaait providers describe <PROVIDER_ID>` lists the setup fields, and
`yaait <COMMAND> --help` lists command options.

## Reports

```sh
yaait usage                             # Human summary, the default
yaait usage --details                   # All metrics
yaait --format json usage --details     # Structured report with all metrics
yaait usage --tracker copilot-work      # One subscription
yaait usage --refresh                   # Fetch fresh data
```

The default view shows primary budgets and reset times when available:

```text
copilot-personal:
    used: 5,647 of 10,000 requests (56.5%)
    resets_at: 2026-10-01 00:00:00Z (in 15 days, 14 hours)
litellm-work:
    used: $7.60 of $40.00 (19.0%)
    resets_at: 2026-10-01 00:00:00Z (in 15 days, 14 hours)
```

Each tracker caches reports for five minutes. `observed_at` records the actual
fetch time, including for cached results. Unknown budgets stay unknown.

JSON output is one schema-versioned document. Human output goes to stdout;
warnings and errors go to stderr. Exit statuses are `0` for success, `1` for
failure, and `2` for partial usage results.

## Automated setup

Pass a JSON object through stdin to add a tracker without interactive prompts:

```sh
printf '%s' '{"token":"…"}' | yaait add \
  --provider github-copilot --input - copilot-work
```

See each provider guide for its input fields. The `providers describe` command
also lists them. Tracker settings and credentials live in platform data
directories. Run `yaait debug` to see the data and cache paths.
