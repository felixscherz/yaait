# Multi-instance tracker core

## Problem and goals

Most AI usage trackers model a provider as a singleton. That prevents a user from
tracking two subscriptions from the same provider, such as separate work and
personal GitHub Copilot accounts.

`yaait` will instead treat every configured tracker as an independently named
instance of a provider implementation. The first release will provide a fast,
local Rust CLI with a stable JSON interface suitable for humans, scripts, and AI
agents. GitHub Copilot will be the first real provider and must prove that two
accounts can be configured and queried in the same invocation.

The goals are:

- support any number of tracker instances per provider;
- give every instance isolated configuration, credentials, state, and cache;
- make provider implementations extensible without coupling their setup or data
  acquisition strategy to the CLI;
- return stable, self-describing JSON by default, including enough provider
  metadata for an AI agent to decide whether a provider fits its task;
- allow interactive human setup and deterministic non-interactive setup through
  the same provider-declared contract;
- fetch enabled trackers concurrently while retaining per-instance failures;
- keep the core reusable by a future local HTTP server.

### Non-goals

- An HTTP server, browser UI, remote service, telemetry, or account sync.
- Automatic discovery through `yaait infer`; the design must leave room for it,
  but v1 does not implement it.
- Runtime-loaded third-party plugins or a stable Rust/ABI plugin SDK. Providers
  are compiled into the binary in v1.
- Implementing Codex, Claude Code, or every provider supported by `aitracker`.
  They are follow-up implementations of the provider contract.
- Aggregating unlike provider metrics into a synthetic global quota.
- Historical usage storage or charting beyond provider-owned cache/state needed
  to produce the current report.
- Cloud-grade secret management. V1 protects local credential files with strict
  filesystem permissions; OS keychain support can be added later.

## Current system

The `yaait` repository is empty at the time of this specification.

The neighboring `../aitracker` Rust CLI is useful prior art: it uses Clap,
Tokio, Reqwest, Serde, concurrent provider requests, XDG-style paths, and JSON
output. Its central `Provider` enum, `[[providers]]` configuration, global
environment-variable/CLI credential discovery, and dispatch function identify a
provider solely by provider kind. In particular, Copilot resolves one token from
`GITHUB_TOKEN` or `gh auth token`. Its fixed `primary`, `secondary`, and
`tertiary` quota fields also encode presentation assumptions into the core model.

`yaait` should reuse the proven implementation techniques, not those singleton
or fixed-metric assumptions.

## Requirements

1. A tracker instance has a globally unique user-chosen `id`, a provider ID, a
   name, an optional description, an enabled flag, timestamps, provider-owned
   public settings, and provider-owned secrets.
2. Multiple tracker instances may use the same provider. Adding, changing,
   disabling, removing, or querying one instance must not affect another.
3. Tracker IDs must match `[a-z][a-z0-9-]{0,62}`. IDs are immutable because they
   determine storage paths and are the stable machine-facing identity.
4. Provider IDs use the same slug syntax and identify compiled-in provider
   implementations. `github-copilot` is the first supported provider ID.
5. The normative add form is
   `yaait add --provider github-copilot github-copilot-work`. The final positional
   argument is the tracker instance ID, not an account or provider ID. When
   `--name` is absent, the name defaults to the ID unchanged.
6. `add` must reject an existing tracker ID, an unknown provider, invalid setup
   input, failed credential validation, and an unsafe storage path without
   partially registering the tracker. `add` is the only multi-file operation
   that provides an all-or-nothing registration guarantee.
7. Provider setup declares its required inputs and which inputs are secret.
   Interactive callers are prompted for missing fields; non-interactive callers
   receive a structured `setup_required` error listing missing fields.
8. Machine callers can supply setup input as one JSON object on stdin using
   `--input -`. Secrets must never be accepted as command-line flag values,
   printed in output, or included in error details.
9. `setup <tracker-id>` reruns setup for an existing instance. The provider must
   validate all new input before the core starts writing. The manifest and
   credentials are then replaced as separate atomic files, so the complete
   reconfiguration is not crash-atomic. If interruption or a storage failure
   leaves an unusable instance, the supported recovery is `yaait remove --yes
   <tracker-id>` followed by `yaait add`.
10. Every tracker receives dedicated data and cache directories derived from its
    ID. Provider code receives scoped paths and must not use global provider
    credential locations unless explicitly configured for that tracker.
11. `usage` queries all enabled tracker instances by default and accepts one or
    more `--tracker <id>` filters. Explicit filters must still name enabled
    trackers. Results identify both the tracker instance and provider.
12. Tracker queries run concurrently with a bounded concurrency limit and an
    individual timeout. Output ordering is deterministic (lexical tracker ID),
    independent of completion order.
13. A failure in one tracker does not discard successful results from others.
    A mixed result is reported as partial success in JSON and with a distinct
    non-zero exit status.
14. Usage reports expose a list of numeric metrics rather than fixed quota slots.
    Providers may report quotas, balances, cumulative counters, or gauges without
    changing the core schema.
15. All ordinary command output is one JSON document on stdout by default.
    Diagnostics and interactive prompts go to stderr or `/dev/tty`; secrets are
    never echoed. `--format text` is optional presentation sugar and must consume
    the same core response objects.
16. JSON responses include a top-level schema version, command name, success or
    partial status, data, warnings, and structured errors. Additive fields are
    allowed within schema version 1; removals, renames, or semantic changes need
    a new major schema version.
17. Expected failures must not panic. Each error has a stable code, message,
    optional tracker/provider identity, and non-secret details.
18. `add` writes a complete private temporary instance directory and renames it
    into place. `setup`, `enable`, and `disable` replace each changed file with
    create-then-rename semantics, but do not promise an atomic transaction across
    files. `remove` deletes the data and cache directories on a best-effort basis
    and may be retried after a partial failure.
19. On Unix, the application data root and tracker directories are owner-only
    (`0700`) and credential files are owner read/write only (`0600`). Existing
    credential files with broader permissions cause a structured security error;
    `yaait` must not silently continue.
20. Provider HTTP clients enforce HTTPS, set finite connect/request timeouts, do
    not log authorization material, and classify authentication, rate-limit,
    transport, timeout, and response-format failures.
21. GitHub Copilot setup accepts a distinct token for each instance, validates it
    against the Copilot usage endpoint, and stores it only in that
    instance's credential file.
22. Given two valid Copilot instances, one `usage` invocation returns two
    independently identified reports. Invalid credentials for one produce one
    success plus one error rather than masking either result.
23. The library layer must not depend on terminal rendering or process exit. A
    future HTTP adapter must be able to call the same registry, setup, and usage
    services used by the CLI.
24. State-changing commands take one application-wide advisory writer lock.
    Another state-changing command fails with `operation_in_progress` while the
    lock is held. Read-only commands do not take the lock. Running read-only and
    state-changing commands concurrently is outside the v1 consistency contract.
25. During unfiltered discovery, malformed manifests, unsupported manifest
    versions, unsafe entries, and manifests for unavailable providers produce
    structured warnings and are otherwise ignored. Healthy instances remain
    usable.

## Proposed design

### Vocabulary

- **Provider**: a compiled-in implementation type, such as `github-copilot`.
  It defines metadata, setup requirements, validation, and report collection.
- **Tracker instance**: one configured subscription/account using a provider,
  such as `github-copilot-work`. It owns an isolated directory and credentials.
- **Registry**: discovers persisted tracker instances and maps each instance's
  provider ID to a provider implementation.
- **Report**: one timestamped observation from one tracker instance.

Keeping provider and tracker IDs distinct is the primary invariant of the
architecture.

### Architecture and responsibilities

Start as one Cargo package with a library and binary rather than a multi-crate
workspace. Module boundaries are API boundaries and can become crates if a real
consumer requires it:

```text
src/
  lib.rs
  main.rs                  CLI parsing, JSON/text emission, exit mapping
  application/
    add.rs                 setup validation and instance registration
    usage.rs               selection, concurrency, timeout, collation
    lifecycle.rs           list, inspect, setup, enable, disable, remove
  domain/
    provider.rs            provider contracts and descriptor types
    tracker.rs             persisted tracker metadata
    report.rs              provider-neutral metric/report model
    error.rs               stable domain error taxonomy
  infrastructure/
    registry.rs            filesystem-backed instance discovery
    storage.rs             safe paths, permissions, atomic reads/writes
    lock.rs                one advisory lock for state-changing commands
    http.rs                shared configured Reqwest client factory
  providers/
    github_copilot.rs      first provider implementation
  presentation/
    json.rs                versioned response envelope
    text.rs                optional human renderer
```

The library exposes application services returning typed values. `main.rs`
turns those values into a response envelope and is the only layer that chooses
an exit code.

The provider registry is an explicit map from provider ID to `Arc<dyn
TrackerProvider>`. It is assembled at startup from compiled-in providers. There
is no enum-based central dispatch and no global mutable provider state.

### Control and data flow

#### Add/setup

1. Acquire the application writer lock. Return `operation_in_progress` if
   another state-changing command holds it.
2. Parse and validate tracker ID and provider ID.
3. Resolve the provider descriptor and setup schema.
4. Parse the optional JSON object and prompt for required missing fields when a
   controlling TTY is available.
5. If required values remain absent, return `setup_required` without writing.
6. Give the complete values to the provider, which separates public settings
   from secrets and validates credentials or network access. Validation may use
   the network but must not write instance state.
7. For `add`, create a private temporary sibling directory, write and sync the
   manifest and credential file, then rename the directory to the final tracker
   directory without replacing an existing directory.
8. For `setup`, create and rename each replacement file after validation. These
   file commits are individually atomic but the pair is not.
9. Return a redacted `TrackerDetail` and release the lock.

The application creates the cache directory after a successful registration or
on first use. A cache creation failure does not invalidate the registration.
`setup` returns `storage_error` when a replacement fails. Because a partial
replacement may no longer be usable, `remove` must be able to delete an instance
by validated directory name without first loading a valid manifest.

#### Usage

1. Scan tracker directories. Convert invalid, unsafe, unsupported, and
   unavailable-provider entries into warnings and exclude them from ordinary
   discovery.
2. Select enabled instances or explicit `--tracker` values.
3. Resolve each instance's provider implementation.
4. Run `collect` jobs through a bounded Tokio concurrency mechanism and apply a
   per-tracker timeout after a job acquires a concurrency permit.
5. Convert each outcome into either a `TrackerReport` or `TrackerError`.
6. Sort reports, errors, and tracker-scoped warnings by tracker ID and emit one
   response envelope.

If an explicit filter names an invalid registry entry, return its specific
registry error as a command-level failure and send no tracker requests. The same
rule applies to `show`. Unfiltered `list` and `usage` emit a warning and continue.

Provider implementations may call remote APIs, parse logs, invoke a local
command, or read provider-specific cached state. Those choices stay behind the
same contract.

### Interfaces and contracts

The following Rust shapes are normative in responsibility and semantics;
implementers may adjust names or ownership details without collapsing the
provider/instance boundary.

```rust
#[async_trait]
pub trait TrackerProvider: Send + Sync {
    fn descriptor(&self) -> ProviderDescriptor;

    async fn validate_setup(
        &self,
        ctx: &SetupContext,
        input: SetupInput,
    ) -> Result<PreparedSetup, TrackerError>;

    async fn collect(
        &self,
        ctx: &TrackerContext,
    ) -> Result<UsageReport, TrackerError>;
}

pub struct SetupContext {
    pub tracker_id: TrackerId,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub http: reqwest::Client,
}

pub struct TrackerContext {
    pub tracker: TrackerManifest,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub credentials: SecretMap,
    pub http: reqwest::Client,
}

pub struct PreparedSetup {
    pub public_settings: serde_json::Map<String, serde_json::Value>,
    pub secrets: SecretMap,
}
```

The core owns persistence of `PreparedSetup`; a provider never writes secrets
directly during setup. During collection, provider-owned cache/state writes are
allowed only below its supplied instance directories.

A `ProviderDescriptor` contains `id`, `name`, `description`, `setup`, and
`metrics`. `setup` is the provider's `SetupSchema`. `metrics` describes the
numeric metrics the provider may return, with `id`, `label`, `description`,
`kind`, and `unit`. A provider may omit a described metric from a report when the
upstream source does not return it. This compact descriptor is the v1 provider
discovery contract. There is no recommendation or ranking API.

`SetupSchema` contains fields with `key`, `label`, `description`, `kind`,
`required`, and optional `allowed_values`. `kind` is one of `string`, `secret`,
`path`, `boolean`, or `choice`. Field keys are unique provider-owned slugs.
Unknown input keys, wrong JSON types, invalid choice values, and an empty value
for a required string or secret produce `invalid_input`. JSON input wins over
prompted input. When a controlling TTY exists, the CLI prompts only for required
fields absent from the JSON object. Otherwise, missing required fields produce
`setup_required`, whose details contain only their keys. `setup` is a full
replacement: it does not merge omitted values or secrets from the existing
instance. Schema output never includes current secret values.

V1 setup is a single request and response. Browser and device authorization are
out of scope.

The persisted manifest is normative JSON:

```json
{
  "schema_version": 1,
  "id": "github-copilot-work",
  "provider": "github-copilot",
  "name": "GitHub Copilot Work",
  "description": null,
  "enabled": true,
  "created_at": "2026-09-14T08:00:00Z",
  "updated_at": "2026-09-14T08:00:00Z",
  "settings": {}
}
```

The credential document is also versioned. Its `secrets` object is opaque to the
core except during setup and collection:

```json
{
  "schema_version": 1,
  "secrets": {
    "token": "stored-secret"
  }
}
```

Storage follows platform directories (shown with Unix/XDG names):

```text
$XDG_DATA_HOME/yaait/
  trackers/
    github-copilot-work/
      tracker.json
      credentials.json
      state/
    github-copilot-personal/
      tracker.json
      credentials.json
      state/
$XDG_CACHE_HOME/yaait/
  trackers/
    github-copilot-work/
    github-copilot-personal/
```

There is deliberately no central instance index in v1. The registry scans
tracker directories, making each directory self-contained and avoiding
cross-file registration transactions. The directory name must equal the
manifest ID. The registry does not follow symlinked tracker directories,
manifests, credential files, state directories, or cache directories. It
preserves unknown fields from a supported manifest version during
read-modify-write. It rejects an unknown future schema version without mutation.

The writer lock is `$XDG_DATA_HOME/yaait/.write.lock` on Unix and the equivalent
platform data path elsewhere. The lock file is `0600` on Unix. `add`, `setup`,
`enable`, `disable`, and `remove` hold an exclusive advisory lock for their
entire operation. Lock acquisition does not wait.

The normative report model is:

```rust
pub struct UsageReport {
    pub observed_at: DateTime<Utc>,
    pub identity: Option<Identity>,
    pub metrics: Vec<UsageMetric>,
    pub attributes: serde_json::Map<String, serde_json::Value>,
}

pub struct UsageMetric {
    pub id: String,
    pub label: String,
    pub kind: MetricKind,       // quota | balance | counter | gauge
    pub unit: String,           // request, token, usd, percent, etc.
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub limit: Option<f64>,
    pub value: Option<f64>,
    pub period: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
    pub attributes: Map<String, Value>,
}
```

`Identity` has optional `account`, `organization`, and `plan` string fields. At
least one field must be present when an identity is returned.

All metric numbers must be finite. Metric IDs and units use the provider ID slug
syntax. Each report contains at most one metric with a given ID. The valid value
fields are:

- `quota`: at least one of `used`, `remaining`, or `limit`; optional `period` and
  `resets_at`; no `value`;
- `balance`: `value`; no `used`, `remaining`, or `limit`;
- `counter`: `value`; optional `period` and `resets_at`; no `used`, `remaining`,
  or `limit`;
- `gauge`: `value`; no `used`, `remaining`, `limit`, `period`, or `resets_at`.

Limits must be positive. Used and remaining values must be non-negative. The
core rejects a report containing invalid or duplicate metrics as
`invalid_provider_response`; it does not silently omit them. Providers return
the direct upstream values when available and do not derive rounded values.

Every normal JSON response uses this envelope:

```json
{
  "schema_version": 1,
  "command": "usage",
  "ok": true,
  "partial": false,
  "data": {},
  "warnings": [],
  "errors": []
}
```

Envelope fields obey these rules:

- complete success: `ok: true`, `partial: false`, and an empty `errors` array;
- mixed `usage` result: `ok: false`, `partial: true`, with at least one tracker
  report and at least one error;
- failure, including an all-failed `usage`: `ok: false`, `partial: false`, and a
  non-empty `errors` array. Command-level failures use `data: null`; an
  all-failed `usage` uses `{ "trackers": [] }`;
- warnings never change `ok`, `partial`, or the exit status;
- an empty unfiltered `usage` is a complete success.

Warnings and errors share a small structure: `code`, `message`, and optional
`tracker_id`, `provider`, and `details`. Details are JSON objects with
code-specific machine data. Credentials, authorization headers, submitted
secret fields, and upstream response bodies are forbidden from every response
field. Messages may change for clarity; codes and detail keys are the stable
machine contract.

The command and successful data shapes are:

| Command | `command` | `data` |
| --- | --- | --- |
| `providers list` | `providers.list` | `{ "providers": [ProviderSummary] }` |
| `providers describe` | `providers.describe` | `{ "provider": ProviderDescriptor }` |
| `add` | `add` | `{ "tracker": TrackerDetail }` |
| `list` | `list` | `{ "trackers": [TrackerSummary] }` |
| `show` | `show` | `{ "tracker": TrackerDetail }` |
| `setup` | `setup` | `{ "tracker": TrackerDetail }` |
| `enable` | `enable` | `{ "tracker": TrackerDetail }` |
| `disable` | `disable` | `{ "tracker": TrackerDetail }` |
| `remove` | `remove` | `{ "removed": { "id": string } \| null }` |
| `usage` | `usage` | `{ "trackers": [TrackerReport] }` |

`ProviderSummary` contains `id`, `name`, and `description`.
`ProviderDescriptor` adds `setup` and `metrics` as defined above.
`TrackerSummary` contains `id`, `provider`, `name`, `description`, `enabled`,
`created_at`, and `updated_at`. `TrackerDetail` adds public `settings`; it never
contains credentials or secret-presence hints. Arrays of providers, trackers,
reports, errors, and tracker-scoped warnings use lexical ID order.
Discovery warnings that have no valid tracker ID use lexical directory-entry
order after tracker-scoped warnings.

`TrackerReport` contains `id`, `provider`, `name`, `description`, `observed_at`,
`identity`, `metrics`, and `attributes`.

V1 reserves these error codes: `invalid_input`, `unknown_provider`,
`tracker_not_found`, `tracker_exists`, `tracker_disabled`, `setup_required`,
`authentication_failed`, `rate_limited`, `timeout`, `transport_error`,
`invalid_provider_response`, `invalid_manifest`, `unsupported_schema`,
`insecure_permissions`, `operation_in_progress`, and `storage_error`.
Provider-specific errors use a provider-prefixed code only when none of the
shared codes fits.

Exit status is `0` for complete success, `1` for any non-partial failure, and `2`
for partial success. The JSON fields, not the exit status, are the primary
machine contract.

Invalid arguments and unrecognized commands emit the JSON envelope with
`invalid_input` and exit 1. The envelope uses the canonical command name when it
can be determined and `cli` otherwise. `--help` and `--version` are the only
exceptions: they print text to stdout and exit 0.

Initial commands are:

```text
yaait providers list
yaait providers describe <provider-id>
yaait add --provider <provider-id> [--name <name>] [--description <text>] [--input -] <tracker-id>
yaait list
yaait show <tracker-id>
yaait setup [--input -] <tracker-id>
yaait enable <tracker-id>
yaait disable <tracker-id>
yaait remove [--yes] <tracker-id>
yaait usage [--tracker <tracker-id>]...
```

`list` includes enabled and disabled instances. `usage` with no configured
trackers succeeds with an empty array. Before starting collection, explicit
filters are validated as a set. An unknown or disabled requested tracker causes
a command-level failure and no tracker requests are sent. `remove` asks for
confirmation on a TTY and requires `--yes` in non-interactive use; after
confirmation it removes the instance data and cache directories. `remove` is a
recovery command as well as a lifecycle command: it operates from a validated
tracker ID even when the manifest or credentials are missing or malformed. It
succeeds when at least one directory existed and neither remains afterward. It
returns `tracker_not_found` when neither existed at the start. A deletion error
returns `storage_error`; the caller may run `remove` again to clear any remaining
directory. A declined interactive removal is a complete success. Its data is
`{ "removed": null }`.

`created_at` and `updated_at` are UTC RFC 3339 timestamps. `add` sets both to the
same injected clock value. A successful `setup`, `enable`, or `disable` changes
`updated_at`; it preserves `created_at`, name, description, and all state/cache
directories. Enabling an enabled tracker or disabling a disabled tracker is a
successful no-op and does not change `updated_at`.

`providers describe` exposes the setup schema so an agent can construct one
non-interactive `add` request. Its metric descriptors and plain description tell
the agent what the provider can measure. For example, the Copilot descriptor
states that it reports Copilot request quota for the account represented by the
supplied GitHub token. V1 does not rank providers or infer a provider from a
task.

Example Copilot setup input:

```json
{"token":"ghu_example"}
```

The returned JSON must never echo `token`. The Copilot adapter calls
`https://api.github.com/copilot_internal/user` with these initial headers:

```text
Authorization: token <token>
Accept: application/json
Editor-Version: vscode/1.96.2
Editor-Plugin-Version: copilot-chat/0.26.7
User-Agent: GitHubCopilotChat/0.26.7
X-GitHub-Api-Version: 2025-04-01
```

The endpoint and compatibility header values are unstable adapter details. They
may change without changing yaait's provider or output schema, and users cannot
override them through tracker settings.

The adapter recognizes `copilot_plan`, `quota_reset_date`, and the `chat` and
`premium_interactions` entries under `quota_snapshots`. It reports these as
`chat` and `premium-interactions` quota metrics with unit `request` and period
`month`. Each uses upstream `remaining`, a numeric `entitlement` as `limit`,
`percent_remaining` in attributes, and the parsed reset time when present. It
omits an unavailable upstream field rather than deriving it. `copilot_plan` maps
to `identity.plan`.

HTTP 401 is `authentication_failed`. HTTP 429, or HTTP 403 with `Retry-After` or
`X-RateLimit-Remaining: 0`, is `rate_limited`. Other HTTP 403 responses are
`authentication_failed`. Other non-success statuses are
`invalid_provider_response`; safe details may include the numeric HTTP status
but not the response body.

`--format json` is accepted as an explicit alias for the default. `--pretty`
changes whitespace only. `--format text` may omit provider-specific attributes,
but must not change collection behavior.

### Migration and compatibility

There is no `yaait` data to migrate. Importing `aitracker` configuration is out
of scope because singleton environment-derived credentials cannot reliably
identify multiple accounts. Future import/inference should propose instance IDs
and use the same add/setup service rather than writing manifests directly.

Manifest and output schemas both start at version 1 but evolve independently.
Persisted data must be version-checked before mutation. Provider-specific
settings may evolve through provider-owned migrations invoked by the registry;
no migration framework is needed until the first such change.

## Decisions and rationale

- **Provider type and tracker instance are separate concepts.** This removes the
  singleton assumption at the domain boundary rather than patching it in auth
  lookup or CLI flags.
- **Instances are filesystem-discovered and self-contained.** This naturally
  isolates accounts and eliminates consistency problems between a central list
  and per-instance data.
- **Provider implementations are compiled in for v1.** A Rust plugin ABI, WASM,
  or subprocess protocol would add packaging and security decisions unrelated to
  proving multi-account tracking. The trait remains the internal extension seam.
- **JSON is the default and canonical presentation.** Text rendering is a view
  over typed responses, so AI integrations do not scrape terminal prose.
- **Metrics are a list, not named slots.** Providers differ substantially in
  quota dimensions, balances, counters, and reset periods.
- **Setup is schema-described and validation-first.** It supports a hidden TTY
  prompt and one-shot JSON automation without forcing all providers to discover
  credentials globally. Validation completes before writes, but v1 deliberately
  accepts weak crash consistency across the manifest and credential files.
- **Secrets are local files in v1.** Owner-only permissions meet the local CLI
  goal and keep the storage contract portable. A keychain backend can later
  implement the same secret-store boundary.
- **Partial usage is a first-class result.** A tracker dashboard is more useful
  when one expired credential does not hide all healthy accounts.
- **One writer is enough.** An application-wide advisory lock keeps modifying
  commands from overlapping without adding per-tracker lock management. V1 does
  not promise consistent reads during a write.
- **Broken registry entries do not disable healthy trackers.** Default discovery
  reports them as warnings and skips them. Explicit access still returns the
  underlying error.
- **Provider discovery stays descriptive.** Provider descriptions and expected
  metrics give an agent enough information to choose a provider. Ranking,
  inference, and recommendation logic remain outside v1.

An ADR is not required before implementation because the repository has no
existing architecture. If runtime-loaded provider plugins are later proposed,
their trust, versioning, and execution boundary should be decided in an ADR.

## Implementation constraints

- Use stable Rust and Tokio. Prefer Serde types over constructing JSON ad hoc.
- Keep domain/application modules independent of Clap, terminal detection,
  formatting, and `std::process::exit`.
- Use one configured Reqwest client factory with redaction-safe tracing and
  provider-appropriate headers added inside adapters.
- Do not read credentials from process-global environment variables or local
  provider CLIs in v1. Setup accepts values only from its JSON input or hidden
  interactive prompts.
- Do not serialize `Secret`, `SecretMap`, or prepared setup types through a
  general debug/JSON path. Secret wrapper debug output must be redacted.
- Resolve all instance paths from a validated `TrackerId`; reject path
  separators, dot components, symlinked tracker/cache directories, symlinked
  manifests/credential files, and paths escaping the application roots.
- Acquire the application writer lock before checking or changing persisted
  state. Release it on every returned error. A stale lock file is harmless
  because ownership comes from the operating system lock, not file existence.
- Default collection concurrency is 8 and the default per-tracker request
  timeout is 15 seconds. These constants should be injected into the usage
  service in tests and may become global configuration later.
- Keep provider results independent. Cross-account aggregation belongs in a
  later application service, not inside a provider adapter.
- Make time, application directories, HTTP transport, and provider registry
  injectable at their stable seams so tests do not depend on a real home
  directory, clock, or network.
- Production code has no endpoint override for Copilot. Adapter tests may inject
  a transport or a test-only base URL that permits loopback HTTP; persisted
  settings and production clients must enforce HTTPS.

## Verification

- Unit-test tracker/provider ID validation, path containment, secret redaction,
  schema serialization, unknown schema versions, metric validation, and error
  serialization.
- Test storage against temporary directories: permission modes on Unix,
  duplicate add, atomic add, individual setup-file replacement, setup recovery
  through remove, malformed manifests, symlink rejection, writer lock
  contention, and independent mutation of two same-provider instances.
- Test application services with fake providers: setup field discovery,
  interactive/non-interactive missing input, unknown and mistyped input,
  validation before writes, selection, bounded concurrency, timeouts,
  deterministic ordering, malformed-registry warnings, and partial results.
- Test the GitHub Copilot adapter with a mock HTTP server and captured fixtures:
  full/partial responses, omitted quota fields, invalid JSON, 401, 403, 429,
  timeout, and redaction of upstream error content.
- CLI integration tests run the compiled binary with isolated XDG directories
  and assert stdout as parsed JSON, stderr separation, exit statuses, pretty
  output equivalence, the response shape of every command, and that secret input
  never appears in either stream.
- The principal acceptance test creates `github-copilot-work` and
  `github-copilot-personal` with distinct mock tokens, runs one `usage` command,
  and verifies two independent API requests and two correctly identified
  reports.
- A second end-to-end test makes one Copilot token fail and verifies one report,
  one tracker-scoped error, `partial: true`, and exit status 2.
- Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` in CI.

## Open questions

- Should a later release prefer the OS keychain over local credential files?
  This does not block v1 because secret persistence is behind a dedicated
  boundary and the local-file behavior is specified.
- Should runtime provider extensibility use subprocesses, WASM, or a Rust plugin
  ABI? This does not block v1; compiled-in providers are the explicit initial
  scope.
