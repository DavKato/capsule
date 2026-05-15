# `result`

Final event in the stream. Emitted once.

## Key fields

| Field             | Type   | Description                          |
| ----------------- | ------ | ------------------------------------ |
| `subtype`         | string | `"success"` or `"error"`             |
| `is_error`        | bool   | Whether the session errored          |
| `duration_ms`     | number | Wall-clock duration                  |
| `duration_api_ms` | number | Time spent in API calls              |
| `num_turns`       | number | Total assistant message count        |
| `result`          | string | Final text output                    |
| `session_id`      | string | Session identifier                   |
| `total_cost_usd`  | number | Total cost across all models         |
| `terminal_reason` | string | `"completed"`, `"interrupted"`, etc. |

## `usage` (cumulative across all turns)

**All token fields are sums across every turn, not point-in-time snapshots.**

```json
{
  "input_tokens": 5,
  "cache_creation_input_tokens": 9452,
  "cache_read_input_tokens": 56286,
  "output_tokens": 605,
  "iterations": [{ "...last turn only..." }]
}
```

`iterations` contains only the **last** iteration, not all turns.

## `modelUsage` (cumulative, per-model)

**Also cumulative sums across all turns**, keyed by model ID. Uses camelCase.

```json
{
	"claude-opus-4-6": {
		"inputTokens": 5,
		"outputTokens": 605,
		"cacheReadInputTokens": 56286,
		"cacheCreationInputTokens": 9452,
		"costUSD": 0.102368,
		"contextWindow": 200000,
		"maxOutputTokens": 64000
	}
}
```

| Field                      | Description                                     |
| -------------------------- | ----------------------------------------------- |
| `inputTokens`              | Sum of non-cached input tokens across all turns |
| `cacheCreationInputTokens` | Sum of cache-creation tokens across all turns   |
| `cacheReadInputTokens`     | Sum of cache-read tokens across all turns       |
| `outputTokens`             | Sum of output tokens across all turns           |
| `costUSD`                  | Total cost for this model                       |
| `contextWindow`            | Context window size                             |
| `maxOutputTokens`          | Max output tokens                               |

## Cumulative cache tokens vs point-in-time context

Each turn re-sends the full conversation. Tokens cached on turn N become
cache-reads on turn N+1. Summing cache token fields across turns
double/triple-counts them.

For point-in-time context window usage, use the per-turn `usage` from the
last `assistant` event — not the cumulative fields here.

Fields that are **not** cumulative (safe to use directly):

- `contextWindow` — static per model
- `maxOutputTokens` — static per model
- `costUSD` / `total_cost_usd` — correctly aggregated, no double-counting
