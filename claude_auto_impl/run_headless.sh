#!/bin/bash
# Claude Code autonomous runner - runs on PLAN.md until Claude outputs [[ALL DONE]]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLAN_FILE="$SCRIPT_DIR/PLAN.md"

cd "$SCRIPT_DIR/.."

PROMPT="Implement the next uncompleted task from $PLAN_FILE.

1. Read $PLAN_FILE, find next uncompleted task
2. If all done, output \`[[ALL DONE]]\` and stop
3. Implement the task following CLAUDE.md rules
4. Run: cargo nextest run -p lumos && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
5. Mark task complete in PLAN.md
6. Commit: git commit -am \"<description>\"

One task per run. Output \`[[ALL DONE]]\` when no tasks remain."

OUTPUT_FILE="/tmp/claude_output_$$"
export OUTPUT_FILE

# Create the Python parser script
PARSER=$(cat << 'PYEOF'
import sys, json, os

output_file = os.environ.get("OUTPUT_FILE", "/tmp/claude_output")
found_all_done = False

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue

    with open(output_file, "a") as f:
        f.write(line + "\n")

    # Check for ALL DONE in raw line
    if "[[ALL DONE]]" in line:
        found_all_done = True

    try:
        data = json.loads(line)
        msg_type = data.get("type")

        if msg_type == "assistant":
            for item in data.get("message", {}).get("content", []):
                if item.get("type") == "text":
                    text = item.get("text", "")
                    print(text, flush=True)
                    if "[[ALL DONE]]" in text:
                        found_all_done = True
                elif item.get("type") == "tool_use":
                    name = item.get("name", "?")
                    inp = item.get("input", {})
                    if "Bash" in name:
                        cmd = inp.get("command", "")[:80]
                        print(f"[{name}] {cmd}", flush=True)
                    elif "Read" in name or "Edit" in name or "Write" in name:
                        fpath = inp.get("file_path", "")
                        print(f"[{name}] {fpath}", flush=True)
                    else:
                        print(f"[{name}]", flush=True)
        elif msg_type == "result":
            print("\n--- Turn complete ---\n", flush=True)

    except json.JSONDecodeError:
        print(line, flush=True)
    except Exception:
        pass

# Exit with code 42 if ALL DONE was found
if found_all_done:
    sys.exit(42)
PYEOF
)

while true; do
    rm -f "$OUTPUT_FILE"

    # Stream JSON and parse with Python for real-time output
    claude --dangerously-skip-permissions --print --output-format stream-json --verbose "$PROMPT" 2>&1 | \
    python3 -u -c "$PARSER"

    PARSER_EXIT=$?

    # Check if Python found ALL DONE (exit code 42)
    if [ $PARSER_EXIT -eq 42 ]; then
        rm -f "$OUTPUT_FILE"
        echo "All tasks complete!"
        break
    fi

    # Also check file as backup
    if grep -q '\[\[ALL DONE\]\]' "$OUTPUT_FILE" 2>/dev/null; then
        rm -f "$OUTPUT_FILE"
        echo "All tasks complete!"
        break
    fi

    rm -f "$OUTPUT_FILE"
    sleep 2
done
