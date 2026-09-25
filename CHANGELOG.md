# Changelog

All notable changes to this project will be documented in this file.

## [0.4.0] - 2026-09-25

### Added

- Add DeepSeek balance provider (#3)
- Add OpenRouter key usage provider (#4)
- Add Codex subscription provider (#5)
- Add Claude Code subscription provider (#6)

### Documentation

- Update README.md
- Publish provider guides with GitHub Pages
- Update guidance on releasing a new version

### Fixed

- Better error message when onboarding provider without giving an id
- Display percent instead of percents

## [0.3.0] - 2026-09-17

### Added

- Show used budget amounts in human reports
- Default scheme-less LiteLLM URLs to HTTPS
- Allow explicitly configured HTTP LiteLLM origins
- Show credential validation progress and seed usage caches
- Support self-update

### Miscellaneous

- Squash into help
- Release v0.3.0

## [0.2.5] - 2026-09-16

### Documentation

- AGENTS.md

### Miscellaneous

- Improve CLI --help
- Release v0.2.5

## [0.2.4] - 2026-09-16

### Fixed

- Clamp usage percentage to sensible range

### Miscellaneous

- Use consistent names in examples
- Release v0.2.4

## [0.2.3] - 2026-09-16

### Miscellaneous

- Fix testing of release artifacts
- Release v0.2.3

## [0.2.2] - 2026-09-16

### Miscellaneous

- Fix windows release workflow
- Release v0.2.2

## [0.2.1] - 2026-09-16

### Fixed

- Use fs2 primitive to check for blocking lock

### Miscellaneous

- Support windows
- Release v0.2.1

## [0.2.0] - 2026-09-15

### Miscellaneous

- Prepare cargo-dist for prebuilt binaries
- Release v0.2.0
- Set release version to v0.2.0

## [0.1.0] - 2026-09-15

### Added

- POC implementation for github-copilot
- Support for GHE
- Better onboarding for github enterprise
- Support litellm
- Plans to implement claude code and codex support
- Debug command to find app directories
- Filter secondary metrics
- Presentation format for humans
- Cache usage for 5 minutes (default)
- --human shortcut for --format human
- Make --human the default
- Homebrew support

### Documentation

- Update documentation
- Update CONTRIBUTING.md

### Miscellaneous

- Release management
- Fix release pipeline
- Release v0.1.0
