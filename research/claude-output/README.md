# Claude `stream-json` Output Reference

Reference captures of Claude Code's `--output-format stream-json --verbose` output,
split by event type for selective reading.

Captured 2026-05-15 with Claude Code 2.1.142.

## Directory structure

```
claude-output/
├── README.md           ← this file
├── system/             ← init event (tools, model, session)
├── rate-limit/         ← rate limit status
├── assistant/          ← model responses + per-turn usage
├── user/               ← tool results
└── result/             ← final summary, cumulative usage, modelUsage
```

Each directory contains:

- `output.json` — compacted sample output
- `reference.md` — field documentation

## Event sequence

```
system (init) → rate_limit_event → [assistant ↔ user]* → result
```

## Quick guide: which directory to read

| You need                                | Read                                     |
| --------------------------------------- | ---------------------------------------- |
| Per-turn token usage (context window %) | `assistant/`                             |
| Cumulative cost / total tokens          | `result/`                                |
| Context window size                     | `result/` (`modelUsage.*.contextWindow`) |
| Available tools / model                 | `system/`                                |
| Why cache tokens inflate totals         | `result/reference.md`                    |

## Regenerating the samples

Run the prompt, capture raw output, then split and compact:

```sh
# 1. Capture raw output
claude --verbose --output-format stream-json \
  -p "$(cat .capsule-test/prompts/chatter.md)" \
  --allowedTools 'Bash(command:echo *)' 'Read' \
  2>/dev/null > /tmp/claude-raw.jsonl

# 2. Split, compact, and overwrite
python3 research/claude-output/split-compact.py /tmp/claude-raw.jsonl
```

The script `split-compact.py` handles:

- **system**: trims `tools`, `skills`, `agents` arrays to 3 samples + count
- **assistant**: collapses streamed chunks with the same `message.id` into one event
- **user**: truncates file content in tool results to ~200 chars
- **rate-limit** and **result**: kept as-is
