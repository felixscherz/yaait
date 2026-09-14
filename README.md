# yaait

`yaait` is a proof-of-concept, JSON-first CLI for tracking multiple AI provider
accounts independently. A tracker is a named instance of a compiled-in provider;
two trackers can therefore use GitHub Copilot with different credentials without
sharing state.

```sh
cargo run -- providers describe github-copilot
printf '%s' '{"token":"…"}' | cargo run -- add \
  --provider github-copilot --input - github-copilot-work
cargo run -- usage
```

`usage` reports only each tracker's primary budget metrics by default. Pass
`--details` to include all usage counters and provider-specific metrics:

```sh
cargo run -- usage --details
```

For a GitHub Enterprise account, also provide its domain or HTTPS URL. The tracker uses
the corresponding `api.<host>` Copilot endpoint and keeps the URL in the tracker
manifest:

```sh
printf '%s' '{"token":"…","enterprise_url":"https://octocorp.ghe.com"}' | \
  cargo run -- add --provider github-copilot --input - github-copilot-work
```

Without `--input`, interactive setup presents a deployment selector that defaults
to GitHub.com. Choosing GitHub Enterprise prompts for a domain such as
`octocorp.ghe.com` or a full URL before prompting for the token.

## LiteLLM

The `litellm` provider reports spend, token counts, request counts, and an
optional key or user budget. It needs a LiteLLM virtual key that can read its own
key information, user information, and aggregated spend data:

```sh
cargo run -- add --provider litellm litellm-work
```

Interactive setup prompts for `https://ai.exxeta.info`, the virtual key, and the
reporting window without placing the key in shell history.
`window` is optional and accepts `7d`, `30d`, or `90d`; it defaults to `30d`.
The tracker uses `/key/info` to identify the key owner and read any key-level
budget, `/v2/user/info` as a fallback for the user's budget, then
`/user/daily/activity/aggregated` for usage totals. Some LiteLLM deployments
require the virtual key to have the `get_spend_routes` permission. Browser and
SSO session credentials are not supported.

Tracker manifests and credentials live in platform data directories. On Linux,
setting `XDG_DATA_HOME` and `XDG_CACHE_HOME` gives a fully isolated environment:

```sh
export XDG_DATA_HOME="$PWD/.local/data"
export XDG_CACHE_HOME="$PWD/.local/cache"
```

The default output is one schema-versioned JSON document. Complete success exits
with status 0, failure with 1, and a mixed usage result with 2.

To inspect the application data and cache directories selected by the current
environment and platform configuration, run:

```sh
cargo run -- debug
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
