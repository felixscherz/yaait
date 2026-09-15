# LiteLLM usage tracker

## Problem and goals

The LiteLLM dashboard shows useful account-level usage, but `yaait`
cannot query it alongside other AI subscriptions. The tracker should expose the
same core totals through the existing provider-neutral report model without
depending on a browser session.

The first implementation will:

- authenticate with a LiteLLM virtual key;
- support independently configured LiteLLM instances and accounts;
- report spend, token counts, and request counts for a fixed recent window;
- report the virtual key's current budget when the proxy provides one;
- work with the LiteLLM 1.100.0 API exposed at `https://my-litellm-instance.example`;
- keep detailed request logs and prompt content out of `yaait` output.

### Non-goals

- Reusing the dashboard's browser or SSO session.
- Downloading individual spend-log rows.
- Reproducing the full LiteLLM dashboard or its charts.
- Admin-wide reporting across unrelated users or teams.
- Changing LiteLLM permissions, budgets, keys, or configuration.

## Findings

The target proxy publishes an OpenAPI document at `/openapi.json`. The Usage
page calls `GET /user/daily/activity/aggregated`. In version 1.100.0 this
endpoint returns `results` grouped by day and a `metadata` object containing:

- `total_spend`;
- prompt, completion, cache-read, cache-creation, and total token counts;
- total, successful, and failed request counts;
- pagination metadata fixed to one page.

`GET /key/info` accepts the caller's virtual key and returns its owner, alias,
team, accumulated spend, maximum budget, budget duration, and next reset time.
The proxy accepts the standard `Authorization: Bearer <key>` header. Its
OpenAPI security declaration also names `x-litellm-api-key`.

LiteLLM may restrict spend routes. A non-admin virtual key needs access to its
own spend data. Deployments can grant `get_spend_routes` when their default role
does not already allow that access.

## Setup contract

Provider ID: `litellm`

Setup fields:

| Field | Kind | Required | Meaning |
| ----- | ---- | -------- | ------- |
| `base_url` | string | yes | HTTPS origin of the LiteLLM proxy |
| `token` | secret | yes | LiteLLM virtual key |
| `window` | choice | no | Reporting window: `7d`, `30d`, or `90d`; default `30d` |

Setup validation will:

1. trim and normalize `base_url`, removing a trailing slash;
2. reject credentials embedded in the URL, query strings, and fragments;
3. require HTTPS;
4. call `/key/info` with the supplied virtual key;
5. derive the caller's `user_id` from the response when present;
6. call `/user/daily/activity/aggregated` for the requested window;
7. store only the normalized URL, selected window, user ID, key alias, and team
   ID as public settings;
8. store the virtual key in the tracker's credential document.

The tracker will pass `user_id` when one is known. This keeps non-admin queries
explicitly scoped to the key owner. If the proxy authenticates the key but
denies the aggregated endpoint, setup fails with an authorization error rather
than silently falling back to incomplete data.

## Collection contract

Collection performs two independent reads:

1. `/key/info` for current key identity and budget state;
2. `/user/daily/activity/aggregated` for the configured recent window.

The date range includes the current local day. The request passes the local
timezone offset and `include_current_utc_day=true`, matching the dashboard's
handling of local and UTC date boundaries.

The report may contain these metrics:

| ID | Kind | Unit | Source |
| -- | ---- | ---- | ------ |
| `spend` | counter | `usd` | `metadata.total_spend` |
| `prompt-tokens` | counter | `token` | `metadata.total_prompt_tokens` |
| `completion-tokens` | counter | `token` | `metadata.total_completion_tokens` |
| `cache-read-tokens` | counter | `token` | `metadata.total_cache_read_input_tokens` |
| `cache-creation-tokens` | counter | `token` | `metadata.total_cache_creation_input_tokens` |
| `total-tokens` | counter | `token` | `metadata.total_tokens` |
| `requests` | counter | `request` | `metadata.total_api_requests` |
| `successful-requests` | counter | `request` | `metadata.total_successful_requests` |
| `failed-requests` | counter | `request` | `metadata.total_failed_requests` |
| `budget` | quota | `usd` | `/key/info` spend and maximum budget |

All analytics counters carry the configured window in `period`. `budget` is
omitted when the key has no positive maximum budget. When present, it uses the
key's budget duration and reset timestamp.

Identity fields map as follows:

- `account`: key alias, falling back to user ID;
- `organization`: team alias, falling back to team ID;
- `plan`: omitted.

Raw user IDs and team IDs may also appear in report attributes when useful, but
the tracker must never expose the plaintext virtual key, its hash, or spend-log
rows.

## Errors

- HTTP 401 becomes `authentication_failed`.
- HTTP 403 becomes `authorization_failed`.
- HTTP 429 becomes `rate_limited`.
- Request timeout becomes `timeout`.
- Other transport failures become `transport_error`.
- A successful response that cannot be parsed becomes
  `invalid_provider_response`.

Error details may include the HTTP status. They must not contain response bodies
or credentials.

## Acceptance criteria

- `providers describe litellm` declares every setup field and metric.
- Setup rejects malformed or insecure base URLs before sending a request.
- Setup validates both the virtual key and access to aggregated self-usage.
- A stored tracker exposes the configured period totals and current budget.
- Missing optional response fields omit metrics rather than inventing values.
- Authentication, authorization, rate-limit, transport, and malformed-response
  failures use stable error codes and never leak response bodies.
- Mock-server tests cover setup, collection, missing budget, and major error
  classes.
- The CLI integration tests cover provider discovery and redacted tracker
  details.
- Formatting, strict Clippy, and all tests pass.

## Open questions

- Whether virtual keys already allow self-service spend routes must be
  verified with a real user key. The public OpenAPI document cannot answer this.
- Exact monetary accounting eventually needs a decimal representation. The v1
  report uses `f64`, so this implementation follows the existing numeric
  contract.

## Implementation result

Implemented in `src/providers/litellm.rs` and registered in the CLI. Automated
coverage verifies setup, collection, missing budgets, malformed data, status
classification, URL validation, and credential redaction. Formatting, strict
Clippy, and the complete test suite pass. Live verification against
`https://my-litellm-instance.example` remains pending because it requires a
user-supplied virtual key.
