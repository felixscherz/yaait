# OpenRouter

OpenRouter reports API key spend and remaining key budget in USD through its
[current key endpoint](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-key).
Add a tracker with an OpenRouter API key:

```sh
yaait add --provider openrouter openrouter-personal
```

For automated setup, supply the `token` field. Use a separate tracker ID for
each key:

```sh
printf '%s' '{"token":"…"}' | yaait add --provider openrouter --input - openrouter-personal
printf '%s' '{"token":"…"}' | yaait add --provider openrouter --input - openrouter-work
```

The summary shows lifetime spend and the key budget when available. Human
output shows the used share; JSON includes the remaining amount. `--details`
adds daily, weekly, monthly, and external BYOK spend when returned.

The key budget is not the account credit balance. A remaining key budget does
not establish that the account has funds. Missing limits stay unknown. The API
may report reset periods, but yaait does not infer a reset timestamp. Key
labels can contain key fragments, so reports omit them and retain tracker IDs
and available creator and organization IDs.
