# Kimi (Moonshot AI)

Kimi reports the available balance of a pay-as-you-go API account from the
Kimi Open Platform (Moonshot AI). **Kimi Code subscription quotas are not
supported**; they use a separate service and different keys.

```sh
yaait add --provider kimi kimi-personal
yaait usage --tracker kimi-personal
yaait usage --tracker kimi-personal --details
```

Setup requires `token` (your Moonshot API key) and accepts optional `region`:

| Region | Endpoint | Currency |
| --- | --- | --- |
| `international` (default) | `https://api.moonshot.ai/v1/users/me/balance` | USD |
| `china` | `https://api.moonshot.cn/v1/users/me/balance` | CNY |

Choose the region where your key was created. International and China keys
are not interchangeable. OpenCode's `moonshotai` connection is this API
service; yaait does not automatically import its credentials.

For noninteractive setup, pass JSON on stdin using `--input -`:

```json
{"token": "YOUR_API_KEY", "region": "international"}
```

Add another tracker ID (for example, `kimi-work`) for a different account.
Multiple keys for the same account report the same account-wide balance,
not independent key budgets.

The default view shows available balance. `--details` adds voucher and cash
balances when supplied by the API. Negative cash balances are preserved as
debt; a zero or negative available balance means inference is unavailable.
Missing detailed amounts remain unknown, not zero. Usage, spending limits,
reset times, and account identity are not supplied by this endpoint.

Reports are cached for five minutes. Use `--refresh` for a fresh balance.
JSON reports retain the fetched observation time and use stable currency-specific
metric IDs: `balance-usd`, `voucher-balance-usd`, `cash-balance-usd` (or `-cny`).

API references: [International](https://platform.moonshot.ai/docs/api/balance)
and [China](https://platform.moonshot.cn/docs/api/balance).
