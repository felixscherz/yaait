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

## Releasing

1. Decide on the new version, `vX.X.X`.
2. Update the `version` in `Cargo.toml` to `X.X.X` and regenerate `Cargo.lock`:

   ```sh
   cargo generate-lockfile
   ```

3. Generate the release notes:

   ```sh
   git-cliff --tag vX.X.X > CHANGELOG.md
   ```

4. Review the changes and run the development checks above. Then commit the
   version bump and release notes:

   ```sh
   git add Cargo.toml Cargo.lock CHANGELOG.md
   git commit -m "chore: release vX.X.X"
   ```

5. Tag that commit and push both the commit and tag:

   ```sh
   git tag vX.X.X
   git push origin HEAD
   git push origin vX.X.X
   ```

## Documentation

Edit the Markdown files in `docs/` and update `mkdocs.yml` when adding a page.
Preview the site locally with:

```sh
python3 -m pip install -r docs-requirements.txt
python3 -m mkdocs serve
```

Run `python3 -m mkdocs build --strict` before submitting documentation changes.
The documentation workflow builds pull requests and publishes `main` to GitHub
Pages. Set the repository's Pages build source to GitHub Actions.
