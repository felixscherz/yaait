# Install

## Homebrew on macOS and Linux

Once the personal Homebrew tap is available:

```sh
brew install felixscherz/tap/yaait
```

## Shell installer on macOS and Linux

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/felixscherz/yaait/releases/latest/download/yaait-installer.sh | sh
```

## PowerShell installer on Windows

```powershell
irm https://github.com/felixscherz/yaait/releases/latest/download/yaait-installer.ps1 | iex
```

The installers download prebuilt binaries from the latest GitHub Release.
Release archives cover Apple Silicon and Intel macOS, ARM64 and x86_64 Linux,
and ARM64 and x86_64 Windows.

## From source

With Rust 1.85 or newer, clone the repository and install:

```sh
git clone https://github.com/felixscherz/yaait.git
cd yaait
cargo install --locked --path .
```

Continue with [adding a tracker](usage.md).
