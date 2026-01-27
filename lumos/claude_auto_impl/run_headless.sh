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

while true; do
    claude --dangerously-skip-permissions --print "$PROMPT" 2>&1 | tee /tmp/claude_output_$$

    if grep -q '\[\[ALL DONE\]\]' /tmp/claude_output_$$; then
        rm -f /tmp/claude_output_$$
        echo "Done!"
        break
    fi

    rm -f /tmp/claude_output_$$
    sleep 2
done
