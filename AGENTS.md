# yaait

yaait means "yet another ai tracker". It is a CLI application that tracks AI
usage across providers and reports how much usage remains.

## Core design

Multiple subscriptions per provider are a core requirement. A user may have
personal and work GitHub Copilot subscriptions, or several accounts and API
keys for the same API provider. Every subscription must be independently
onboarded, configured, queried, and identified in reports.

A provider implements the Tracker interface that knows how to gather usage
information for that provider. In this Rust codebase, that interface is the
`TrackerProvider` trait in `src/domain/provider.rs`. A tracker is a named,
configured instance of a provider, identified by its own `TrackerId`. Several
trackers can share a `ProviderId`.

- Key tracker configuration, credentials, cache, and lifecycle operations by
  tracker ID. Never assume a provider has only one account or subscription.
- Keep provider-specific setup, authentication, API calls, and interpretation
  of usage in the provider implementation.
- Return the shared `UsageReport` model so the application and renderers can
  handle providers consistently. Preserve provider-specific details through
  the model's attributes when needed.
- Keep tracker identity available in reports so a caller can tell which
  subscription has the remaining budget.

## Humans and agents

yaait has two main end users, humans and agents.

The default human report is for a quick glance. Show aggregated usage and
primary budget metrics in readable form, including remaining usage and reset
information when available. Leave detailed counters and provider metadata out
of the default view. Preserve subscription identity when summarizing usage,
and only aggregate values whose units, scope, and reporting periods are
compatible.

Agents may use the human report to choose which subscription, API account, or
plan to route requests to based on the remaining budget. When they need more
detail or structured data, provide JSON with stable tracker and metric IDs,
units, usage limits, remaining amounts, observation timestamps, reset times,
and available account or plan identity.

In the current CLI, output format and metric detail are separate choices:

```sh
yaait usage                             # Human summary, the default
yaait usage --details                   # Human report with all metrics
yaait --format json usage --details     # Structured report with all metrics
```

- Treat JSON as a public interface. Preserve the versioned response envelope
  and account for compatibility when changing its schema or field semantics.
- Preserve unknown or unavailable values as unknown. Do not turn a missing
  budget into zero remaining usage or an unlimited allowance.
- Keep `observed_at` tied to the actual data fetch, including cached reports.
  Routing decisions depend on knowing how fresh the data is.
- Preserve successful tracker reports when another tracker fails. Keep
  failures attributable to the affected tracker.
- Keep human data on stdout and human warnings and errors on stderr. JSON
  output must remain one parseable response document. Preserve exit statuses
  of 0 for success, 1 for failure, and 2 for partial usage results.
- Support setup through JSON input without interactive prompts when stdin is
  not a terminal. Never expose credentials in reports, errors, or logs.

yaait supplies usage information for routing decisions. The caller chooses
where to send requests.

## Code organization

- `src/domain/` defines provider contracts, tracker identity, usage reports,
  and errors.
- `src/providers/` implements individual providers.
- `src/application/` coordinates tracker lifecycle and usage collection.
- `src/infrastructure/` handles persistence, credentials, caching, locking,
  and HTTP support.
- `src/presentation/` renders human and JSON responses.
- `src/main.rs` handles CLI arguments, interactive input, and output.
- `tests/cli.rs` exercises the CLI.

When changing a provider or report, verify behavior with multiple trackers for
the same provider. For reporting changes, check both human and JSON output,
including primary and detailed views. Use mocked provider responses and
isolated data directories in tests rather than real credentials or accounts.

## Development checks

Follow `CONTRIBUTING.md`. Before submitting changes, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test
pre-commit run --all-files
```

Review any changes made by hooks and rerun them until they pass. Use
conventional commit messages, as required by the repository hooks. Update
`README.md` when changing commands or user-visible behavior.
