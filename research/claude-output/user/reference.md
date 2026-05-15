# `user`

Emitted once per tool result returned to the model.

## Key fields

| Field                           | Type     | Description                            |
| ------------------------------- | -------- | -------------------------------------- |
| `message.content[]`             | object[] | Tool result blocks                     |
| `message.content[].tool_use_id` | string   | Matches the `tool_use` block's `id`    |
| `message.content[].type`        | string   | `"tool_result"`                        |
| `message.content[].content`     | string   | Text content of the tool result        |
| `timestamp`                     | string   | ISO 8601 timestamp                     |
| `tool_use_result`               | object   | Structured result metadata (see below) |

## `tool_use_result` variants

For file reads:

```json
{
	"type": "text",
	"file": {
		"filePath": "/path/to/file",
		"content": "...",
		"numLines": 30,
		"startLine": 1,
		"totalLines": 625
	}
}
```

For bash commands:

```json
{
	"stdout": "hello from the test agent",
	"stderr": "",
	"interrupted": false,
	"isImage": false,
	"noOutputExpected": false
}
```

## Notes

- The output.json in this directory truncates file content to ~200 chars since the content itself is not structurally relevant.
