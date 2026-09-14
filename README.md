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

Tracker manifests and credentials live in platform data directories. On Linux,
setting `XDG_DATA_HOME` and `XDG_CACHE_HOME` gives a fully isolated environment:

```sh
export XDG_DATA_HOME="$PWD/.local/data"
export XDG_CACHE_HOME="$PWD/.local/cache"
```

The default output is one schema-versioned JSON document. Complete success exits
with status 0, failure with 1, and a mixed usage result with 2.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
