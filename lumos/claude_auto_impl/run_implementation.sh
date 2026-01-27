#!/bin/bash

# =============================================================================
# Claude Code Autonomous Implementation Runner
# =============================================================================
# This script runs Claude Code in a loop until all tasks in SPEC.md are complete.
# It handles logging, progress tracking, and graceful interruption.
# =============================================================================

set -euo pipefail

# Configuration
SPEC_FILE="SPEC.md"
PROGRESS_LOG="logs/progress.log"
SESSION_LOG="logs/session_$(date +%Y%m%d_%H%M%S).log"
MAX_ITERATIONS=500
ITERATION_DELAY=3
MAX_CONSECUTIVE_FAILURES=5

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Create necessary directories
mkdir -p logs
mkdir -p src/{scalar,simd,gpu}
mkdir -p benches
mkdir -p tests

# Initialize progress log
echo "=== Implementation Started: $(date) ===" >> "$PROGRESS_LOG"

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
        # Check if there are any unchecked boxes
        if grep -q "\[ \]" "$SPEC_FILE"; then
            return 1  # Not complete
        else
            return 0  # All complete
        fi
    fi
    return 1
}

# Function to count progress
show_progress() {
    if [[ -f "$SPEC_FILE" ]]; then
        local total=$(grep -c "\[.\]" "$SPEC_FILE" 2>/dev/null || echo "0")
        local done=$(grep -c "\[x\]" "$SPEC_FILE" 2>/dev/null || echo "0")
        echo -e "${BLUE}Progress: ${done}/${total} tasks complete${NC}"
    fi
}

# Main prompt for Claude Code
CLAUDE_PROMPT='You are implementing a project according to SPEC.md. Follow these rules strictly:

## WORKFLOW FOR EACH ITERATION:

1. **READ STATE FIRST**
   - Read SPEC.md to find the next unchecked task `[ ]`
   - Read relevant README.md files to understand current state
   - If all tasks show `[x]`, respond with exactly: "ALL_TASKS_COMPLETE"

2. **RESEARCH PHASE** (for new components)
   - Use web search to research current best practices
   - Check for latest API conventions and patterns
   - Verify no deprecated APIs will be used
   - Document findings in relevant README.md

3. **IMPLEMENTATION PHASE**
   - Implement ONE task at a time
   - Follow the public API design principles from SPEC.md
   - Write clean, documented code
   - Ensure no deprecated APIs are used

4. **TESTING PHASE**
   - Write unit tests BEFORE marking task complete
   - Run tests to verify they pass
   - Aim for high coverage of the new code

5. **OPTIMIZATION TASKS** (scalar/simd/gpu)
   - Implement optimization in isolation
   - Create dedicated benchmark in benches/
   - Run benchmark and record results
   - Compare against baseline
   - If optimization shows <5% improvement or regression:
     * Document findings in README.md
     * Remove the optimization code
     * Mark task as complete with note "[SKIPPED - no benefit]"
   - If optimization is beneficial:
     * Keep the code
     * Document speedup in README.md

6. **UPDATE STATUS**
   - Mark completed task with `[x]` in SPEC.md
   - Update README.md in relevant folder with:
     * What was implemented
     * Optimizations used (or why skipped)
     * Benchmark results
     * Any important notes

7. **COMMIT PROGRESS**
   - Git commit after each completed task
   - Use descriptive commit message

## CRITICAL RULES:
- ONE task per iteration only
- ALWAYS run tests before marking complete
- ALWAYS run benchmarks for optimization tasks
- NEVER use deprecated APIs (research first if unsure)
- REMOVE optimizations that do not provide measurable benefit
- Keep all README.md files updated with current status

Begin by reading SPEC.md and implementing the next incomplete task.'

# Initialize git if needed
if [[ ! -d ".git" ]]; then
    echo -e "${BLUE}Initializing git repository...${NC}"
    git init
    echo "logs/" >> .gitignore
    git add .gitignore
    git commit -m "Initial commit: project setup"
fi

echo -e "${GREEN}Starting autonomous implementation...${NC}"
echo -e "${YELLOW}Press Ctrl+C to gracefully stop${NC}\n"

iteration=0
consecutive_failures=0

while [[ $iteration -lt $MAX_ITERATIONS ]]; do
    ((iteration++))
    
    echo -e "\n${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BLUE}Iteration $iteration / $MAX_ITERATIONS - $(date)${NC}"
    echo -e "${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    
    show_progress
    
    # Check if already complete
    if check_completion; then
        echo -e "\n${GREEN}✓ All tasks complete!${NC}"
        echo "=== All Tasks Complete: $(date) ===" >> "$PROGRESS_LOG"
        break
    fi
    
    # Run Claude Code
    echo -e "\n${YELLOW}Running Claude Code...${NC}\n"
    
    # Capture output and check for completion signal
    set +e
    output=$(claude --print "$CLAUDE_PROMPT" 2>&1 | tee -a "$SESSION_LOG")
    exit_code=$?
    set -e
    
    # Log iteration
    echo "--- Iteration $iteration: $(date) ---" >> "$PROGRESS_LOG"
    
    # Check for completion signal in output
    if echo "$output" | grep -q "ALL_TASKS_COMPLETE"; then
        echo -e "\n${GREEN}✓ Claude signaled all tasks complete!${NC}"
        echo "=== All Tasks Complete (signaled): $(date) ===" >> "$PROGRESS_LOG"
        break
    fi
    
    # Check for failures
    if [[ $exit_code -ne 0 ]]; then
        ((consecutive_failures++))
        echo -e "${RED}Iteration failed (${consecutive_failures}/${MAX_CONSECUTIVE_FAILURES})${NC}"
        echo "FAILED: Iteration $iteration" >> "$PROGRESS_LOG"
        
        if [[ $consecutive_failures -ge $MAX_CONSECUTIVE_FAILURES ]]; then
            echo -e "${RED}Too many consecutive failures. Stopping.${NC}"
            echo "=== Stopped due to failures: $(date) ===" >> "$PROGRESS_LOG"
            exit 1
        fi
    else
        consecutive_failures=0
        echo "SUCCESS: Iteration $iteration" >> "$PROGRESS_LOG"
    fi
    
    # Brief delay between iterations
    sleep $ITERATION_DELAY
done

if [[ $iteration -ge $MAX_ITERATIONS ]]; then
    echo -e "\n${YELLOW}Maximum iterations reached.${NC}"
    echo "=== Max iterations reached: $(date) ===" >> "$PROGRESS_LOG"
fi

echo -e "\n${GREEN}Session complete. Check logs/ for details.${NC}"
show_progress
