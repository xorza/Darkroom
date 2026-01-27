#!/bin/bash

# =============================================================================
# Claude Code Autonomous Implementation Runner - Lumos Project
# =============================================================================
# Runs Claude Code in a loop until all tasks in SPEC.md are complete.
# Adapted for lumos astrophotography stacking project.
# =============================================================================

set -euo pipefail

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
SPEC_FILE="$SCRIPT_DIR/SPEC.md"
PLAN_FILE="$PROJECT_ROOT/PLAN.md"
LOG_DIR="$SCRIPT_DIR/logs"
PROGRESS_LOG="$LOG_DIR/progress.log"
SESSION_LOG="$LOG_DIR/session_$(date +%Y%m%d_%H%M%S).log"
MAX_ITERATIONS=200
ITERATION_DELAY=5
MAX_CONSECUTIVE_FAILURES=3

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Create log directory
mkdir -p "$LOG_DIR"

# Initialize progress log
echo "=== Implementation Started: $(date) ===" >> "$PROGRESS_LOG"
echo "Project: lumos" >> "$PROGRESS_LOG"
echo "Working directory: $PROJECT_ROOT" >> "$PROGRESS_LOG"

# Trap for graceful shutdown
cleanup() {
    echo -e "\n${YELLOW}Interrupted. Saving state...${NC}"
    echo "=== Session Interrupted: $(date) ===" >> "$PROGRESS_LOG"
    exit 0
}
trap cleanup SIGINT SIGTERM

# Function to check if all tasks are complete
check_completion() {
    if [[ -f "$SPEC_FILE" ]]; then
        if grep -q "\[ \]" "$SPEC_FILE"; then
            return 1  # Not complete
        else
            return 0  # All complete
        fi
    fi
    return 1
}

# Function to count and show progress
show_progress() {
    if [[ -f "$SPEC_FILE" ]]; then
        local total=$(grep -cE "^\s*- \[.\]" "$SPEC_FILE" 2>/dev/null || echo "0")
        local done=$(grep -cE "^\s*- \[x\]" "$SPEC_FILE" 2>/dev/null || echo "0")
        local skipped=$(grep -c "\[SKIPPED\]" "$SPEC_FILE" 2>/dev/null || echo "0")
        echo -e "${CYAN}Progress: ${done}/${total} tasks complete (${skipped} skipped)${NC}"
    fi
}

# Function to get next task
get_next_task() {
    if [[ -f "$SPEC_FILE" ]]; then
        grep -m1 "^\s*- \[ \]" "$SPEC_FILE" | sed 's/.*\[ \] //' || echo "No tasks found"
    fi
}

# Main prompt for Claude Code
CLAUDE_PROMPT="You are implementing the lumos astrophotography stacking library.

## CRITICAL: Read These Files First
1. Read \`$SPEC_FILE\` to find the next unchecked task \`[ ]\`
2. Read \`$PROJECT_ROOT/CLAUDE.md\` for coding rules
3. Read \`$PLAN_FILE\` for algorithm details and context
4. Read relevant \`NOTES-AI.md\` files in the module you're working on

## WORKFLOW FOR EACH TASK:

1. **FIND NEXT TASK**
   - Read SPEC.md, find the next \`[ ]\` task
   - If all tasks show \`[x]\`, respond with exactly: \"ALL_TASKS_COMPLETE\"

2. **RESEARCH** (if needed)
   - Use web search for best practices
   - Check online documentation
   - Document findings in relevant NOTES-AI.md

3. **IMPLEMENT**
   - ONE task at a time only
   - Follow CLAUDE.md coding rules strictly:
     * Use \`.unwrap()\` for infallible operations
     * Use \`.expect(\"message\")\` for non-obvious cases
     * Add \`#[derive(Debug)]\` to structs
     * Prefer crashing on logic errors
   - Write clean, documented code

4. **TEST & VERIFY**
   - Write unit tests
   - Run: \`cargo nextest run -p lumos && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings\`
   - Tests MUST pass before marking complete

5. **BENCHMARK** (for optimization tasks only)
   - Create benchmark in \`benches/\`
   - Run: \`cargo bench -p lumos --features bench --bench <name>\`
   - Save results to \`benches/<name>_results.txt\`
   - If improvement < threshold: document why, remove code, mark \"[SKIPPED]\"

6. **UPDATE STATUS**
   - Mark completed task with \`[x]\` in SPEC.md
   - Update relevant NOTES-AI.md with what was implemented
   - Update bench-analysis.md if benchmarks were run

7. **GIT COMMIT**
   - Commit changes with descriptive message
   - Format: \"lumos: <brief description of what was done>\"

## RULES:
- ONE task per iteration
- ALWAYS run verification commands before marking complete
- ALWAYS benchmark optimization tasks
- Remove optimizations that don't meet threshold
- Keep NOTES-AI.md files updated
- Follow CLAUDE.md rules exactly

Begin now. Read SPEC.md and implement the next incomplete task."

# Change to project root
cd "$PROJECT_ROOT"

echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${GREEN}  Lumos Autonomous Implementation Runner${NC}"
echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "Project root: ${CYAN}$PROJECT_ROOT${NC}"
echo -e "Spec file:    ${CYAN}$SPEC_FILE${NC}"
echo -e "Log file:     ${CYAN}$SESSION_LOG${NC}"
echo ""
echo -e "${YELLOW}Press Ctrl+C to gracefully stop${NC}"
echo ""

show_progress
echo -e "Next task: ${CYAN}$(get_next_task)${NC}"
echo ""

iteration=0
consecutive_failures=0

while [[ $iteration -lt $MAX_ITERATIONS ]]; do
    ((iteration++))

    echo -e "\n${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}Iteration $iteration / $MAX_ITERATIONS - $(date '+%H:%M:%S')${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"

    show_progress
    echo -e "Next: ${CYAN}$(get_next_task)${NC}"

    # Check if already complete
    if check_completion; then
        echo -e "\n${GREEN}All tasks complete!${NC}"
        echo "=== All Tasks Complete: $(date) ===" >> "$PROGRESS_LOG"
        break
    fi

    # Run Claude Code
    echo -e "\n${YELLOW}Running Claude Code...${NC}\n"

    set +e
    output=$(claude --print "$CLAUDE_PROMPT" 2>&1 | tee -a "$SESSION_LOG")
    exit_code=$?
    set -e

    # Log iteration
    echo "--- Iteration $iteration: $(date) ---" >> "$PROGRESS_LOG"
    echo "Task: $(get_next_task)" >> "$PROGRESS_LOG"

    # Check for completion signal
    if echo "$output" | grep -q "ALL_TASKS_COMPLETE"; then
        echo -e "\n${GREEN}Claude signaled all tasks complete!${NC}"
        echo "=== All Tasks Complete (signaled): $(date) ===" >> "$PROGRESS_LOG"
        break
    fi

    # Check for failures
    if [[ $exit_code -ne 0 ]]; then
        ((consecutive_failures++))
        echo -e "${RED}Iteration failed (${consecutive_failures}/${MAX_CONSECUTIVE_FAILURES})${NC}"
        echo "FAILED: Iteration $iteration (exit code $exit_code)" >> "$PROGRESS_LOG"

        if [[ $consecutive_failures -ge $MAX_CONSECUTIVE_FAILURES ]]; then
            echo -e "${RED}Too many consecutive failures. Stopping.${NC}"
            echo "=== Stopped due to failures: $(date) ===" >> "$PROGRESS_LOG"
            exit 1
        fi
    else
        consecutive_failures=0
        echo "SUCCESS: Iteration $iteration" >> "$PROGRESS_LOG"
    fi

    sleep $ITERATION_DELAY
done

if [[ $iteration -ge $MAX_ITERATIONS ]]; then
    echo -e "\n${YELLOW}Maximum iterations reached.${NC}"
    echo "=== Max iterations reached: $(date) ===" >> "$PROGRESS_LOG"
fi

echo -e "\n${GREEN}Session complete.${NC}"
echo -e "Log saved to: ${CYAN}$SESSION_LOG${NC}"
show_progress
