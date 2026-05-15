#!/usr/bin/env python3
"""Split a claude --verbose --output-format stream-json JSONL file
into per-type compacted JSON files under research/claude-output/."""

import json
import os
import sys

def main():
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <raw-output.jsonl>", file=sys.stderr)
        sys.exit(1)

    src = sys.argv[1]
    out_dir = os.path.dirname(os.path.abspath(__file__))

    with open(src) as f:
        lines = [json.loads(line) for line in f if line.strip()]

    # --- system: trim long arrays ---
    system = next(e for e in lines if e["type"] == "system")
    for key in ["tools", "mcp_servers", "slash_commands", "agents", "skills"]:
        arr = system.get(key)
        if isinstance(arr, list) and len(arr) > 3:
            system[key] = arr[:3] + [f"...{len(arr) - 3} more"]
    plugins = system.get("plugins")
    if isinstance(plugins, list) and len(plugins) > 2:
        system["plugins"] = plugins[:2] + [f"...{len(plugins) - 2} more"]
    write_json(os.path.join(out_dir, "system", "output.json"), system)

    # --- rate-limit ---
    rl = next(e for e in lines if e["type"] == "rate_limit_event")
    write_json(os.path.join(out_dir, "rate-limit", "output.json"), rl)

    # --- assistant: collapse streamed chunks per message.id ---
    merged = {}
    for e in lines:
        if e["type"] != "assistant":
            continue
        mid = e["message"]["id"]
        if mid not in merged:
            merged[mid] = json.loads(json.dumps(e))
        else:
            seen = {
                (b.get("type"), b.get("id", b.get("text", "")[:40]))
                for b in merged[mid]["message"]["content"]
            }
            for block in e["message"]["content"]:
                key = (block.get("type"), block.get("id", block.get("text", "")[:40]))
                if key not in seen:
                    merged[mid]["message"]["content"].append(block)
                    seen.add(key)
    write_json(os.path.join(out_dir, "assistant", "output.json"), list(merged.values()))

    # --- user: truncate file content ---
    user_events = [json.loads(json.dumps(e)) for e in lines if e["type"] == "user"]
    for e in user_events:
        for c in e.get("message", {}).get("content", []):
            if isinstance(c, dict) and isinstance(c.get("content"), str) and len(c["content"]) > 200:
                c["content"] = c["content"][:200] + "\n...[truncated]"
        tur = e.get("tool_use_result", {})
        if isinstance(tur, dict):
            fc = tur.get("file", {})
            if isinstance(fc, dict) and len(fc.get("content", "")) > 200:
                fc["content"] = fc["content"][:200] + "\n...[truncated]"
    write_json(os.path.join(out_dir, "user", "output.json"), user_events)

    # --- result ---
    result = next(e for e in lines if e["type"] == "result")
    write_json(os.path.join(out_dir, "result", "output.json"), result)

    # Report
    original = os.path.getsize(src)
    total = sum(
        os.path.getsize(os.path.join(out_dir, d, "output.json"))
        for d in ["system", "rate-limit", "assistant", "user", "result"]
    )
    print(f"Split {original} bytes -> {total} bytes ({total * 100 // original}%)")
    for d in ["system", "rate-limit", "assistant", "user", "result"]:
        p = os.path.join(out_dir, d, "output.json")
        print(f"  {d}: {os.path.getsize(p)} bytes")


def write_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    main()
