# Claude Code

Claude Code reports subscription usage percentages, reset windows, and extra
usage budget. Add a tracker for each subscription:

```sh
yaait add --provider claude-code claude-code-personal
```

Supply exactly one of `token` or `credential_file`. Tokens must be
subscription OAuth access tokens, not API keys. For automated setup, provide
an absolute path to a credential file:

```sh
printf '%s' '{"credential_file":"/absolute/path/to/.credentials.json"}' | \
  yaait add --provider claude-code --input - claude-code-personal
```

Interactive setup suggests `$CLAUDE_CONFIG_DIR/.credentials.json` or
`~/.claude/.credentials.json` when available. Press Enter to use the suggested
file, or enter another subscription's absolute path. The selected path stays
with that tracker. Automated setup still requires an explicit file or token.

Each tracker can use a different file or token. yaait reads the file on each
fresh fetch, so it picks up tokens refreshed by the upstream CLI. yaait does
not refresh tokens itself. If authentication fails, sign in with
`claude auth login` and retry `yaait usage --refresh`. For a supplied token,
replace it with `yaait setup <TRACKER_ID>`. A failed refresh returns a
tracker-specific error rather than cached usage as fresh data.

Claude Code reads `claudeAiOauth.accessToken` and the available subscription
type. On macOS, Claude Code may store credentials in Keychain. Supply a token
or a private credential file in that case. Keep each file tied to one
subscription.

Usage is measured in percentage points of each independent window, with a
limit of 100. Do not add windows together. Missing windows stay unknown. The
summary includes enabled monthly extra usage when available. Amounts identified
as USD are converted from cents to dollars; other amounts retain their credit
units. Disabled extra usage is omitted. Detailed output includes available
per-model and OAuth app weekly windows.

These subscription endpoints are not a stable public API and may change.
