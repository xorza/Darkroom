#!/bin/bash

# =============================================================================
# Claude Code FULLY AUTONOMOUS Runner - Lumos Project (Headless Mode)
# =============================================================================
# WARNING: This runs WITHOUT any confirmation prompts!
# Only use in isolated environments (containers, VMs, sandboxes)
# =============================================================================

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
SPEC_FILE="$SCRIPT_DIR/SPEC.md"
PLAN_FILE="$PROJECT_ROOT/PLAN.md"
LOG_DIR="$SCRIPT_DIR/logs"
SESSION_LOG="$LOG_DIR/headless_$(date +%Y%m%d_%H%M%S).log"
MAX_TURNS=300
MAX_ITERATIONS=100
ITERATION_DELAY=5

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

mkdir -p "$LOG_DIR"

echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${RED}  FULLY AUTONOMOUS MODE - NO CONFIRMATIONS${NC}"
echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "This will run Claude Code with --dangerouslySkipPermissions"
echo "All file operations will be executed without confirmation."
echo ""
echo -e "Project: ${CYAN}lumos${NC}"
echo -e "Root:    ${CYAN}$PROJECT_ROOT${NC}"
echo ""
read -p "Type 'yes' to continue: " confirm
if [[ "$confirm" != "yes" ]]; then
    echo "Aborted."
    exit 0
fi

CLAUDE_PROMPT="You are implementing the lumos astrophotography stacking library autonomously.

## READ FIRST:
1. \`$SPEC_FILE\` - find next \`[ ]\` task
2. \`$PROJECT_ROOT/CLAUDE.md\` - coding rules (MUST follow)
3. \`$PLAN_FILE\` - algorithm details
4. Relevant \`NOTES-AI.md\` files

## WORKFLOW:

1. **FIND TASK**: Read SPEC.md, find next \`[ ]\`. If all \`[x]\`, output \"ALL_TASKS_COMPLETE\".

2. **RESEARCH** (if needed): Web search, document in NOTES-AI.md.

3. **IMPLEMENT**: One task. Follow CLAUDE.md:
   - \`.unwrap()\` for infallible, \`.expect()\` with message otherwise
   - \`#[derive(Debug)]\` on structs
   - Crash on logic errors, don't swallow

4. **VERIFY**: Run:
   \`\`\`
   cargo nextest run -p lumos && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
   \`\`\`
   Must pass before proceeding.

5. **BENCHMARK** (optimizations only):
   \`\`\`
   cargo bench -p lumos --features bench --bench <name> | tee benches/<name>_results.txt
   \`\`\`
   If <10% improvement: document, remove code, mark \"[SKIPPED]\"

6. **UPDATE**: Mark \`[x]\` in SPEC.md. Update NOTES-AI.md.

7. **COMMIT**: \`git commit -m \"lumos: <description>\"\`

RULES:
- ONE task per response
- Verification MUST pass
- Benchmark all optimizations
- Remove non-beneficial optimizations
- Keep NOTES-AI.md updated

Start now."

cd "$PROJECT_ROOT"

echo -e "\n${GREEN}Starting headless implementation...${NC}"
echo "Logging to: $SESSION_LOG"
echo ""

for ((i=1; i<=MAX_ITERATIONS; i++)); do
    echo -e "${BLUE}[Iteration $i/$MAX_ITERATIONS]${NC} $(date '+%H:%M:%S')"

    # Check completion
    if ! grep -qE "^\s*- \[ \]" "$SPEC_FILE" 2>/dev/null; then
        echo -e "${GREEN}All tasks complete!${NC}"
        break
    fi

    # Show progress
    total=$(grep -cE "^\s*- \[.\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    done=$(grep -cE "^\s*- \[x\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    next=$(grep -m1 "^\s*- \[ \]" "$SPEC_FILE" | sed 's/.*\[ \] //' || echo "none")
    echo -e "Progress: ${CYAN}$done/$total${NC} | Next: ${CYAN}$next${NC}"

    # Run Claude Code in headless mode
    set +e
    claude \
        --dangerouslySkipPermissions \
        --max-turns "$MAX_TURNS" \
        --print \
        "$CLAUDE_PROMPT" 2>&1 | tee -a "$SESSION_LOG"

    result=$?
    set -e

    if [[ $result -ne 0 ]]; then
        echo -e "${YELLOW}Warning: Claude exited with code $result${NC}"
    fi

    # Check for completion signal
    if tail -100 "$SESSION_LOG" | grep -q "ALL_TASKS_COMPLETE"; then
        echo -e "${GREEN}Implementation complete!${NC}"
        break
    fi

    sleep $ITERATION_DELAY
done

echo -e "\n${GREEN}Session finished.${NC}"
echo "Log: $SESSION_LOG"

# Final progress
if [[ -f "$SPEC_FILE" ]]; then
    echo ""
    total=$(grep -cE "^\s*- \[.\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    done=$(grep -cE "^\s*- \[x\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    skipped=$(grep -c "\[SKIPPED\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    echo -e "Final: ${GREEN}$done/$total${NC} complete (${YELLOW}$skipped${NC} skipped)"
fi
