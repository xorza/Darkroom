#!/bin/bash

# =============================================================================
# Claude Code FULLY AUTONOMOUS Runner (Headless Mode)
# =============================================================================
# ⚠️  WARNING: This runs WITHOUT any confirmation prompts!
# Only use in isolated environments (containers, VMs, sandboxes)
# =============================================================================

set -euo pipefail

# Configuration
SPEC_FILE="SPEC.md"
LOG_DIR="logs"
SESSION_LOG="$LOG_DIR/headless_$(date +%Y%m%d_%H%M%S).log"
MAX_TURNS=200  # Maximum API turns per invocation
MAX_ITERATIONS=50
ITERATION_DELAY=5

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

mkdir -p "$LOG_DIR"

echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${RED}⚠️  FULLY AUTONOMOUS MODE - NO CONFIRMATIONS${NC}"
echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "This will run Claude Code with --dangerouslySkipPermissions"
echo "All file operations will be executed without confirmation."
echo ""
read -p "Are you sure you want to continue? (yes/no): " confirm
if [[ "$confirm" != "yes" ]]; then
    echo "Aborted."
    exit 0
fi

CLAUDE_PROMPT='You are implementing a project according to SPEC.md. Execute these steps:

## WORKFLOW:

1. **READ STATE**: Read SPEC.md, find next `[ ]` task. If all `[x]`, output "ALL_TASKS_COMPLETE" and stop.

2. **RESEARCH** (new components): Web search for best practices. Document in README.

3. **IMPLEMENT**: One task only. No deprecated APIs. Clean, documented code.

4. **TEST**: Write and run unit tests. Must pass before proceeding.

5. **BENCHMARK** (optimizations only):
   - Create benchmark
   - Run and record results
   - If <5% improvement: document in README, remove code, mark "[SKIPPED]"
   - If beneficial: keep code, document speedup

6. **UPDATE**: Mark task `[x]` in SPEC.md. Update relevant README.md files.

7. **COMMIT**: Git commit with descriptive message.

RULES:
- ONE task per response
- Tests must pass
- Benchmarks required for optimizations
- Remove non-beneficial optimizations
- Keep READMEs updated

Start now. Read SPEC.md and implement the next incomplete task.'

# Initialize git if needed
if [[ ! -d ".git" ]]; then
    git init
    echo "logs/" >> .gitignore
    git add -A
    git commit -m "Initial commit"
fi

echo -e "\n${GREEN}Starting headless implementation...${NC}"
echo "Logging to: $SESSION_LOG"
echo ""

for ((i=1; i<=MAX_ITERATIONS; i++)); do
    echo -e "${BLUE}[Iteration $i/$MAX_ITERATIONS]${NC} $(date)"
    
    # Check completion
    if ! grep -q "\[ \]" "$SPEC_FILE" 2>/dev/null; then
        echo -e "${GREEN}✓ All tasks complete!${NC}"
        break
    fi
    
    # Show progress
    total=$(grep -c "\[.\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    done=$(grep -c "\[x\]" "$SPEC_FILE" 2>/dev/null || echo 0)
    echo "Progress: $done/$total"
    
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
        echo -e "${GREEN}✓ Implementation complete!${NC}"
        break
    fi
    
    sleep $ITERATION_DELAY
done

echo -e "\n${GREEN}Session finished.${NC}"
echo "Log saved to: $SESSION_LOG"

# Final progress
if [[ -f "$SPEC_FILE" ]]; then
    echo ""
    echo "Final Progress:"
    grep -c "\[x\]" "$SPEC_FILE" || echo "0"
    echo "tasks complete out of"
    grep -c "\[.\]" "$SPEC_FILE" || echo "0"
fi
