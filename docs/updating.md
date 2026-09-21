# Updating

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
