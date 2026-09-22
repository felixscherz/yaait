# Codex

Codex reports 5-hour and weekly subscription limits with reset times. Add a
tracker for each subscription:

```sh
yaait add --provider codex codex-personal
```

Interactive setup asks you to choose an `auth.json` file or an access token,
then prompts for that credential. Supply exactly one of `token` or
`credential_file`. Tokens must be subscription OAuth access tokens, not API
keys. For automated setup, supply an explicit absolute file path:

```sh
printf '%s' '{"credential_file":"/absolute/path/to/auth.json"}' | \
  yaait add --provider codex --input - codex-personal
```

Interactive setup suggests `$CODEX_HOME/auth.json` or `~/.codex/auth.json` when
available. Press Enter to use the suggested file, or enter the absolute path
for another subscription. The selected path is stored in that tracker. Usage
collection does not switch to a newly discovered file. Automated setup still
requires an explicit file or token.

Each tracker can use a different file or token. yaait reads the file on each
fresh fetch, so it picks up tokens refreshed by the upstream CLI. yaait does
not refresh tokens itself. If a token expires or is rejected, sign in with
`codex login` for that subscription and retry `yaait usage --refresh`. For a
supplied token, replace it with `yaait setup <TRACKER_ID>`. A failed refresh
returns a tracker-specific error rather than cached usage as fresh data.

For credential files, Codex reads `tokens.access_token` and
`tokens.account_id` and pins the account during setup. When supplying a token,
add `account_id` if you need to select a workspace.

Usage is measured in percentage points of each independent window, with a
limit of 100. Do not add windows together. Missing windows stay unknown. The
summary also includes the credit balance when available. Credits have no
assumed currency. Detailed output includes code review usage. JSON keeps the
metric IDs `primary` and `secondary` for compatibility; their labels are
`5-hour limit` and `Weekly limit`.
