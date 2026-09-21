# yaait

Yet another AI tracker. `yaait` is a CLI that reports AI usage and remaining
budgets for people and agents. Add multiple named trackers for the same provider
to keep personal and work subscriptions separate.

## Get started

Install `yaait` using the [installation guide](https://felixscherz.github.io/yaait/install/), then add a tracker:

```sh
yaait providers list
yaait add --provider github-copilot copilot-personal
yaait usage
```

The setup prompt asks for the provider's credentials. Give each subscription a
different tracker ID. See the [usage guide](https://felixscherz.github.io/yaait/usage/)
for detailed reports, JSON output, and automated setup.

## Supported providers

| Provider | Reports | Setup guide |
| --- | --- | --- |
| Claude Code | Subscription windows, reset times, and extra usage budget | [Claude Code](docs/providers/claude-code.md) |
| Codex | Subscription windows, reset times, and credits | [Codex](docs/providers/codex.md) |
| DeepSeek | API account balances in USD and CNY | [DeepSeek](docs/providers/deepseek.md) |
| GitHub Copilot | Request quota and reset time | [GitHub Copilot](docs/providers/github-copilot.md) |
| LiteLLM | Spend, tokens, requests, and key or user budget | [LiteLLM](docs/providers/litellm.md) |
| OpenRouter | API key spend and remaining key budget in USD | [OpenRouter](docs/providers/openrouter.md) |

Browse the [documentation](https://felixscherz.github.io/yaait/) for installation,
usage, and updates.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development checks and commit
requirements.
