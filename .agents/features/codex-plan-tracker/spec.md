# Codex plan tracker

## Problem and goals

Codex shows ChatGPT plan limits in its clients, but `yaait` cannot report those
limits beside other subscriptions. OpenAI's Codex app-server now exposes the
needed account methods over a documented JSONL protocol.

The tracker will:

- use the installed Codex CLI as the credential and protocol owner;
- report primary, secondary, and additional metered quota windows;
- report plan identity, available credits, and account token activity when
  supplied;
- support multiple accounts through separate Codex home directories;
- avoid parsing or persisting `auth.json` itself.

### Non-goals

- OpenAI API organization billing and Admin Usage API data.
- Starting login flows from `yaait`.
- Redeeming rate-limit reset credits.
- Sending workspace-owner notifications.
- Running Codex turns or reading conversation content.

## Findings

`codex app-server` uses newline-delimited JSON on stdin and stdout. A client must
send `initialize`, then the `initialized` notification. The relevant documented
methods are:

- `account/read` for account email and plan type;
- `account/rateLimits/read` for quota windows and credits;
- `account/usage/read` for lifetime and daily token activity.

Rate-limit windows contain `usedPercent`, `windowDurationMins`, and a Unix reset
timestamp. The response can contain a backward-compatible primary snapshot and
a map keyed by dynamic `limitId` values.

The usage methods require authentication backed by Codex services. API-key-only
and Bedrock authentication do not represent ChatGPT plan usage.

## Setup contract

Provider ID: `codex-plan`

Setup fields:

| Field | Kind | Required | Meaning |
| ----- | ---- | -------- | ------- |
| `codex_binary` | path | no | Codex executable; default `codex` from `PATH` |
| `codex_home` | path | no | Account-specific Codex home directory |

Setup starts app-server, performs the protocol handshake, calls `account/read`,
and verifies that the account uses a supported ChatGPT-backed authentication
mode. It then calls both read-only usage methods. No secret is copied into the
tracker credential file.

Users can configure multiple accounts by logging in with different
`CODEX_HOME` values and pointing separate trackers at those directories.

## Collection contract

Each collection starts a short-lived app-server process and requests account,
rate-limit, and usage snapshots. It shuts down the process after receiving all
responses.

Quota windows use `limit: 100`, `used: usedPercent`, and
`remaining: 100 - usedPercent`, with unit `percent`. Each metric carries the
upstream limit ID and window duration in attributes. Dynamic upstream IDs must
be converted to valid, collision-safe metric slugs.

The five-hour and weekly quota windows are primary metrics and appear in the
default `usage` response. Other quota windows and activity metrics are details.

Additional metrics may report credit balance, available reset credits,
lifetime tokens, peak daily tokens, and streak lengths when supplied by the
server.

## Process safety

- Add Tokio process and asynchronous I/O support.
- Match responses by request ID and ignore unrelated notifications.
- Bound line length and captured stderr.
- Kill and reap the child on timeout, cancellation, malformed output, or early
  EOF.
- Do not return raw app-server errors when they may contain account data.
- Keep protocol parsing separate from metric conversion so recorded fixtures
  can test both layers.

## Acceptance criteria

- Setup detects a missing executable, failed process, missing login, unsupported
  authentication mode, and malformed protocol response.
- Collection returns stable quota IDs and preserves the original upstream IDs
  in attributes.
- Separate `codex_home` settings can address separate accounts.
- Cancellation does not leave an app-server child running.
- Protocol and metric conversion tests cover single-bucket and multi-bucket
  responses.
- Formatting, strict Clippy, and all tests pass.

## Open questions

- The provider descriptor currently lists a fixed metric set while Codex can
  add dynamic quota buckets. The first implementation may describe the stable
  metrics and document that reports can include additional bucket-derived IDs.
- Very large token counters exceed exact `f64` integer precision. A later report
  schema should add exact integer values.
