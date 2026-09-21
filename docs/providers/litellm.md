# LiteLLM

LiteLLM reports spend, token and request counts, and available key or user
budget from a LiteLLM proxy. Setup needs a proxy URL and a virtual key.

```sh
yaait add --provider litellm litellm-work
```

Automated setup accepts `base_url`, `token`, and optional `window`:

```sh
printf '%s' '{"base_url":"https://ai.example.com","token":"…","window":"30d"}' | \
  yaait add --provider litellm --input - litellm-work
```

The reporting window can be `7d`, `30d`, or `90d`; it defaults to `30d`.
URLs without a scheme use HTTPS. Include `http://` for a local plaintext
proxy. The virtual key must be able to read key information, user information,
and aggregated spend. Some deployments require the `get_spend_routes`
permission. Browser and SSO session credentials are not supported.
