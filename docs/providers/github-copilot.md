# GitHub Copilot

GitHub Copilot reports request quota and reset time for the account represented
by a GitHub token. It supports GitHub.com and GitHub Enterprise.

```sh
yaait add --provider github-copilot copilot-personal
```

Setup asks for a GitHub token authorized for the Copilot usage endpoint. For
GitHub Enterprise, choose the Enterprise deployment during setup and enter its
domain or HTTPS URL.

For automated setup, pass `token` and, for Enterprise, `enterprise_url`:

```sh
printf '%s' '{"token":"…"}' | yaait add --provider github-copilot --input - copilot-personal
printf '%s' '{"token":"…","enterprise_url":"https://octocorp.ghe.com"}' | \
  yaait add --provider github-copilot --input - copilot-work
```

Give each subscription its own tracker ID. The default report shows premium
interaction quota; `--details` also shows chat request quota.
