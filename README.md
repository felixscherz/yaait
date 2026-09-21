# yaait

Yet another AI tracker. `yaait` is a CLI that tracks AI usage and remaining
budgets, with human-readable summaries and JSON output for scripts and agents.

Track multiple instances of the same provider, such as personal and work
GitHub Copilot subscriptions or several LiteLLM keys. Each instance is a named
**tracker** with its own configuration, credentials, and usage cache.

## Getting started

### Install

#### macOS and Linux with Homebrew

Once the personal Homebrew tap is available:

```sh
brew install felixscherz/tap/yaait
```

#### macOS and Linux with the shell installer

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/felixscherz/yaait/releases/latest/download/yaait-installer.sh | sh
```

#### Windows with PowerShell

```powershell
irm https://github.com/felixscherz/yaait/releases/latest/download/yaait-installer.ps1 | iex
```

The installers download prebuilt binaries from the latest GitHub Release.
Release archives cover Apple Silicon and Intel macOS, ARM64 and x86_64 Linux,
and ARM64 and x86_64 Windows.

#### From source

With Rust 1.85 or newer, clone the repository and install:

```sh
git clone https://github.com/felixscherz/yaait.git
cd yaait
cargo install --locked --path .
```

### Add a tracker and check usage

```sh
yaait providers list
yaait add --provider github-copilot copilot-personal
yaait usage
```

Setup prompts for credentials. To track another subscription for the same
provider, give it a different tracker ID:

```sh
yaait add --provider github-copilot copilot-work
```

Run `yaait providers describe <PROVIDER_ID>` for setup requirements, or
`yaait <COMMAND> --help` for command options.

## Supported providers

| Provider ID | Reports | Setup |
| --- | --- | --- |
| `claude-code` | Subscription windows, reset times, and extra usage budget | OAuth token or explicit credential-file path |
| `deepseek` | API account balances in USD and CNY | DeepSeek API key |
| `github-copilot` | Copilot request quota and reset time | GitHub token; supports GitHub.com and GitHub Enterprise |
| `litellm` | Spend, tokens, requests, and available key or user budget | Proxy URL and LiteLLM virtual key |
| `openrouter` | API key spend and remaining key budget in USD | OpenRouter API key |

DeepSeek uses the [user balance endpoint](https://api-docs.deepseek.com/api/get-user-balance).
The summary shows available account balances separately for each currency.
`--details` adds granted and topped-up balances when returned. JSON includes
`is_available`, the provider's indication that the balance permits API calls.
The API does not report spending limits, reset times, or account identity.
Use distinct tracker names for different accounts. Keys for the same account
report the same account balance, so do not add those balances together.
This tracks API credit, not usage in the DeepSeek chat application.

```sh
printf '%s' '{"token":"…"}' | yaait add --provider deepseek --input - deepseek-personal
printf '%s' '{"token":"…"}' | yaait add --provider deepseek --input - deepseek-work
```

OpenRouter uses the [current key endpoint](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key).
The summary shows lifetime spend and the key budget when available.
Human output shows the used share; JSON includes the remaining amount.
`--details` adds daily, weekly, monthly, and external BYOK spend when returned.
The key budget is not the account credit balance; having budget left does not
prove the account has funds. A missing limit stays unknown. Reset periods come
from the API, but no reset timestamp is inferred. Key labels can contain key
fragments, so reports omit them and retain tracker IDs and available creator
and organization IDs.

```sh
printf '%s' '{"token":"…"}' | yaait add --provider openrouter --input - openrouter-personal
printf '%s' '{"token":"…"}' | yaait add --provider openrouter --input - openrouter-work
```

For GitHub Enterprise, choose the Enterprise deployment during setup and enter
its domain or HTTPS URL.

LiteLLM setup accepts a reporting window of `7d`, `30d`, or `90d`, defaulting
to `30d`. URLs without a scheme use HTTPS; include `http://` for a local
plaintext proxy. The virtual key must be able to read key information, user
information, and aggregated spend. Some deployments require the
`get_spend_routes` permission. Browser and SSO session credentials are not
supported.

### Codex subscriptions

```sh
yaait add --provider codex codex-personal
```

Interactive setup asks you to choose an auth.json file or an access token, then
prompts for that credential. Supply exactly one of `token` or `credential_file`.
Tokens must be subscription OAuth access tokens, not API keys. For
noninteractive setup, pass a JSON object with
`"credential_file":"/absolute/path/to/auth.json"` through `--input -`.
Interactive setup suggests an existing credential file from
`$CODEX_HOME/auth.json`, or `~/.codex/auth.json`.

Press Enter to use the suggested file, or enter another subscription's absolute
path. The selected path is stored in that tracker; usage collection never
switches to a newly discovered file. Noninteractive JSON setup still requires
an explicit credential file or token.

Each tracker can use a different file or token. Files are read on each fresh
fetch so refreshed tokens from the upstream CLI take effect. yaait does not
refresh tokens itself. If a fresh request rejects an expired or invalid token,
yaait reports an `authentication_failed` error for that tracker and does not
return its cached usage as fresh data. Use `codex login` for the affected
subscription, then retry `yaait usage --refresh`. For a supplied token, replace
it with `yaait setup <TRACKER_ID>`.
Codex reads `tokens.access_token` and `tokens.account_id` and pins the account
during setup. For a token, also supply `account_id` when selecting a workspace.

Usage is measured in percentage points of each independent window, with a
limit of 100. Windows are not added together. Missing windows remain unknown.
The summary also includes the credit balance when available. Credits have no
assumed currency; missing balances remain unknown. Detailed output includes
code-review usage.

### Claude Code subscriptions

```sh
yaait add --provider claude-code claude-code-personal
```

Supply exactly one of `token` or `credential_file`. Tokens must be subscription
OAuth access tokens, not API keys. For noninteractive setup, pass a JSON object
with `"credential_file":"/absolute/path/to/.credentials.json"` through
`--input -`.
Interactive setup suggests an existing credential file from
`$CLAUDE_CONFIG_DIR/.credentials.json`, or `~/.claude/.credentials.json`.

Press Enter to use the suggested file, or enter another subscription's absolute
path. The selected path is stored in that tracker; usage collection never
switches to a newly discovered file. Noninteractive JSON setup still requires
an explicit credential file or token.

Each tracker can use a different file or token. Files are read on each fresh
fetch so refreshed tokens from the upstream CLI take effect. yaait does not
refresh tokens itself. Failed token authentication produces a tracker-specific
error explaining how to sign in with `claude auth login` and retry
`yaait usage --refresh`. A failed refresh does not return cached usage as fresh
data. For supplied tokens, update the tracker with `yaait setup <TRACKER_ID>`.
Claude Code reads `claudeAiOauth.accessToken` and the available subscription
type. On macOS, Claude Code may store credentials in Keychain; supply a token or
a private credential file instead. Keep each file tied to one subscription.

Usage is measured in percentage points of each independent window, with a
limit of 100. Windows are not added together. Missing windows remain unknown.
The summary also includes enabled monthly extra usage when available. Amounts
identified as USD are converted from cents to dollars. Without a reported
currency, amounts retain their credit units. Missing usage or limits remain
unknown; disabled extra usage is omitted. Utilization is interpreted as a
percentage, including values below one. Detailed output includes available
per-model and OAuth-app weekly windows.

These subscription endpoints are not a stable public API and may change.

## Usage and output

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

### Automated setup

Pass a JSON object through stdin to add a tracker without interactive prompts:

```sh
printf '%s' '{"token":"…"}' | yaait add \
  --provider github-copilot --input - copilot-work
```

For GitHub Enterprise, include `"enterprise_url":"https://octocorp.ghe.com"`.
Use `providers describe` to discover each provider's input fields.

Tracker settings and credentials live in platform data directories. Run
`yaait debug` to see the data and cache paths.

## Updating

For shell or PowerShell installations:

```sh
yaait update                         # Latest stable release
yaait update --check                 # Check without installing
yaait update --version <VERSION>     # Specific release or downgrade
```

The updater verifies checksums and preserves tracker settings, credentials,
and caches. Prereleases require an explicit version. Downgrades do not migrate
application data to an older schema.

For Homebrew, run `brew upgrade felixscherz/tap/yaait`. Source builds and manual
installations use their original installation method. `yaait update --check`
works for these installations too.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development checks and commit
requirements. Install the git hooks with `prek install`.

Releases use `vX.Y.Z` tags matching the version in `Cargo.toml`. Pushing a tag
runs the [release workflow](.github/workflows/release.yml), which builds the
archives and installers and publishes the Homebrew formula. Preview release
notes with `git-cliff --unreleased`.
