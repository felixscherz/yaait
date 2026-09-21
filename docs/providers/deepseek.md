# DeepSeek

DeepSeek reports API account balances in USD and CNY through its
[user balance endpoint](https://api-docs.deepseek.com/api/get-user-balance).
You need a DeepSeek API key for each account you want to track.

```sh
yaait add --provider deepseek deepseek-personal
```

For automated setup, supply the `token` field:

```sh
printf '%s' '{"token":"…"}' | yaait add --provider deepseek --input - deepseek-personal
```

Use a distinct tracker name for another account:

```sh
printf '%s' '{"token":"…"}' | yaait add --provider deepseek --input - deepseek-work
```

The summary shows available balances separately for each currency. `--details`
adds granted and topped-up balances when DeepSeek returns them. JSON includes
`is_available`, DeepSeek's indication that the balance permits API calls.

The API does not report spending limits, reset times, or account identity.
Keys for the same account report the same balance, so do not add those balances
together. This tracks API credit, not usage in the DeepSeek chat application.
