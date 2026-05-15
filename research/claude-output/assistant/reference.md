# `assistant`

Emitted per streamed chunk of each assistant message. Multiple chunks share
the same `message.id`; content blocks accumulate across chunks.

## Key fields

| Field                 | Type     | Description                                                        |
| --------------------- | -------- | ------------------------------------------------------------------ |
| `message.model`       | string   | Model that produced this message                                   |
| `message.id`          | string   | Message ID (same across streamed chunks of one message)            |
| `message.content[]`   | object[] | Content blocks: `thinking`, `text`, `tool_use`                     |
| `message.stop_reason` | string?  | `null` while streaming, `"end_turn"` / `"tool_use"` on final chunk |
| `message.usage`       | object   | **Per-turn** token counts (see below)                              |
| `parent_tool_use_id`  | string?  | Set when this message is inside a sub-agent                        |
| `request_id`          | string   | API request ID                                                     |

## `message.usage` (per-turn, not cumulative)

```json
{
	"input_tokens": 1,
	"cache_creation_input_tokens": 166,
	"cache_read_input_tokens": 22318,
	"output_tokens": 2,
	"cache_creation": {
		"ephemeral_5m_input_tokens": 0,
		"ephemeral_1h_input_tokens": 166
	},
	"service_tier": "standard"
}
```

| Field                         | Description                                |
| ----------------------------- | ------------------------------------------ |
| `input_tokens`                | Non-cached input tokens for this turn only |
| `cache_creation_input_tokens` | Tokens written to cache this turn          |
| `cache_read_input_tokens`     | Tokens read from cache this turn           |
| `output_tokens`               | Output tokens for this turn                |
| `cache_creation.ephemeral_*`  | Breakdown of cache creation by TTL tier    |

### Per-turn context calculation

Total context window usage for a single turn:

```
input_tokens + cache_creation_input_tokens + cache_read_input_tokens + output_tokens
```

`input_tokens` does **not** include cache tokens — they are separate fields.

## Content block types

| `type`     | Key fields                      | Description             |
| ---------- | ------------------------------- | ----------------------- |
| `thinking` | `thinking`, `signature`         | Extended thinking block |
| `text`     | `text`                          | Text output             |
| `tool_use` | `id`, `name`, `input`, `caller` | Tool invocation         |

## Notes

- The `usage` object is **identical** across all streamed chunks of the same message. It reflects the full turn's usage, not a running partial count.
- The output.json in this directory collapses chunks with the same `message.id` into a single event. The raw JSONL has one line per chunk.
- Unlike `result.modelUsage`, per-message `usage` reflects a single API call, not a cumulative sum.
