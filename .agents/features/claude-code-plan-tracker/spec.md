# Claude Code plan tracker

## Problem and goals

Claude Code users can see five-hour and weekly subscription limits in the
interactive client, but `yaait` cannot query them. Claude Code currently reads
those values from an OAuth usage endpoint that Anthropic has not documented as
a public API.

The tracker will provide a deliberately experimental integration that:

- reports current subscription usage windows and reset times;
- supports separate accounts through separately supplied long-lived OAuth
  tokens;
- handles new or missing quota buckets without failing unrelated metrics;
- makes the unsupported upstream dependency clear in provider metadata and
  documentation.

### Non-goals

- Anthropic API organization spend and Admin Usage API reporting.
- Extracting credentials from macOS Keychain, Windows credential storage, or a
  Claude Desktop session.
- Refreshing ordinary short-lived Claude OAuth access tokens.
- Enabling extra usage, buying credits, or changing organization limits.
- Estimating plan percentages from local transcript token counts.

## Findings

Claude Code 2.1.223 calls:

```text
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <OAuth token>
anthropic-beta: oauth-2025-04-20
```

Observed bucket names include `five_hour`, `seven_day`, model-scoped weekly
limits, and overage or extra-usage state. Quota objects contain utilization
percentages and reset timestamps.

The endpoint is not part of Anthropic's public API documentation. It is also
rate-limited more aggressively than inference requests in some accounts. The
provider must treat its response as a versioned compatibility input, not a
stable contract.

Anthropic's documented Admin Usage API reports API organization consumption. It
does not report a personal Claude Pro or Max subscription window.

On macOS, Claude Code stores credentials in Keychain. This machine has a
Keychain credential and no `~/.claude/.credentials.json`, which rules out a
portable file-only discovery scheme.

## Setup contract

Provider ID: `claude-code-plan`

Setup fields:

| Field | Kind | Required | Meaning |
| ----- | ---- | -------- | ------- |
| `oauth_token` | secret | yes | Long-lived Claude Code OAuth token |
| `api_base` | string | no | OAuth API origin; default `https://api.anthropic.com` |

The intended credential source is `claude setup-token`, which creates a token
for subscription-backed automated use. Setup validates the token with the
read-only usage request. It must reject ordinary Anthropic API keys with a clear
authentication error.

## Collection contract

Known usage windows map to stable IDs such as `five-hour`, `seven-day`,
`seven-day-sonnet`, and `seven-day-opus`. Unknown bucket names are converted to
valid, collision-safe slugs and retain their original names in attributes.

Each quota uses `limit: 100`, the returned utilization as `used`, and the
clamped difference as `remaining`. Reset timestamps map to `resets_at`.
Extra-usage balances or spend limits are included only when the response gives
enough data to represent them without inference.

## Compatibility and errors

- Mark the provider experimental in its descriptor.
- Parse known fields with optional values and ignore unknown JSON fields.
- Maintain fixtures for each response shape seen in supported Claude Code
  versions.
- HTTP 401 and 403 become authentication or authorization errors.
- HTTP 429 becomes `rate_limited`; honor a useful `Retry-After` value but do not
  retry in a tight loop.
- Do not return response bodies or tokens in errors.
- A schema change that removes all recognizable usage buckets becomes
  `invalid_provider_response`.

## Acceptance criteria

- Provider metadata identifies the unsupported upstream dependency.
- Setup accepts a valid long-lived subscription OAuth token and rejects an API
  key.
- Known and unknown quota buckets produce valid metric IDs.
- Rate limiting returns promptly without repeated polling.
- Response bodies and credentials never appear in output or debug formatting.
- Mock-server tests cover legacy, current, partial, unauthorized, rate-limited,
  and malformed responses.
- Formatting, strict Clippy, and all tests pass.

## Open questions

- Anthropic may change or remove `/api/oauth/usage` without notice. There is no
  supported replacement today.
- `claude setup-token` should be verified against the usage endpoint before
  implementation. If it cannot read subscription usage, the provider would
  need short-lived token refresh support and should remain deferred.

