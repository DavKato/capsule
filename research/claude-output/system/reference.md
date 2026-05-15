# `system` (subtype: `init`)

First event in the stream. Emitted once.

## Key fields

| Field                 | Type     | Description                                  |
| --------------------- | -------- | -------------------------------------------- |
| `session_id`          | string   | Unique session identifier                    |
| `cwd`                 | string   | Working directory                            |
| `model`               | string   | Primary model ID                             |
| `tools`               | string[] | Available tool names                         |
| `mcp_servers`         | object[] | Connected MCP servers                        |
| `skills`              | string[] | Available slash-command skills               |
| `agents`              | string[] | Available agent types                        |
| `plugins`             | object[] | Active plugins with `name`, `path`, `source` |
| `permissionMode`      | string   | `"default"`, `"plan"`, etc.                  |
| `claude_code_version` | string   | CLI version                                  |
| `memory_paths.auto`   | string   | Path to auto-memory directory                |
| `fast_mode_state`     | string   | `"off"` or `"on"`                            |

## Notes

- The `tools`, `skills`, and `agents` arrays can be large (30+ items). The output.json in this directory is trimmed to 3 samples each; see the root README.md for regeneration instructions.
