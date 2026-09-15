# Contributing

## Development Checks

Run the test suite from the repository root:

```sh
cargo test
```

Run the formatter in check mode and lint all targets with warnings treated as
errors:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Run all pre-commit hooks against the complete worktree:

```sh
pre-commit run --all-files
```

The pre-commit configuration also checks trailing whitespace, end-of-file
formatting, YAML and TOML syntax, and oversized added files. Commit messages
must use the configured conventional-commit format.

## Before Submitting Work

Before submitting a change, agents and contributors must run `cargo test` and
`pre-commit run --all-files`. All pre-commit hooks must pass before the work is
submitted. If a hook modifies a file, review the change and rerun the hooks
until they pass.
