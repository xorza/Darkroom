#!/bin/bash

# =============================================================================
# Claude Code FULLY AUTONOMOUS Runner
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

# Parse arguments
SKIP_CONFIRM=false
SINGLE_ITERATION=false
for arg in "$@"; do
    case $arg in
        --yes|-y)
            SKIP_CONFIRM=true
            ;;
        --single|-1)
            SINGLE_ITERATION=true
            MAX_ITERATIONS=1
            ;;
    esac
done

echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "${RED}  FULLY AUTONOMOUS MODE - NO CONFIRMATIONS${NC}"
echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "This will run Claude Code with --dangerouslySkipPermissions"
echo "All file operations will be executed without confirmation."
echo ""
echo -e "Project: ${CYAN}Scenarium${NC}"
echo -e "Root:    ${CYAN}$PROJECT_ROOT${NC}"
echo ""

if [[ "$SKIP_CONFIRM" != "true" ]]; then
    read -p "Type 'yes' to continue: " confirm
    if [[ "$confirm" != "yes" ]]; then
        echo "Aborted."
        exit 0
    fi
fi

CLAUDE_PROMPT="You are implementing the project autonomously.

## READ FIRST:
1. \`$SPEC_FILE\` - find next \`[ ]\` task
2. \`$PROJECT_ROOT/CLAUDE.md\` - coding rules (MUST follow)
3. \`$PLAN_FILE\` - algorithm details
4. Relevant \`NOTES-AI.md\` files

## WORKFLOW:

1. **FIND TASK**: Read SPEC.md, find next \`[ ]\`. If all tasks are marked \`[x]\`, output exactly \`[[ALL DONE]]\` and stop.

2. **RESEARCH** (if needed): Web search, document in NOTES-AI.md.

3. **IMPLEMENT**: One task. Follow CLAUDE.md:
   - \`.unwrap()\` for infallible, \`.expect()\` with message otherwise
   - \`#[derive(Debug)]\` on structs
   - Crash on logic errors, don't swallow

4. **VERIFY**: Run:
   \`\`\`
   cargo nextest run -p <project> && cargo fmt && cargo check && cargo clippy --all-targets -- -D warnings
   \`\`\`
   Must pass before proceeding.

5. **BENCHMARK** (optimizations only):
   \`\`\`
   cargo bench -p <project> --features bench --bench <name> | tee benches/<name>_results.txt
   \`\`\`
   If <10% improvement: document, remove code, mark \"[SKIPPED]\"

6. **UPDATE**: Mark \`[x]\` in SPEC.md. Update NOTES-AI.md.

7. **COMMIT**: \`git commit -m \"<description>\"\`

RULES:
- ONE task per response
- Verification MUST pass
- Benchmark all optimizations
- Remove non-beneficial optimizations
- Keep NOTES-AI.md updated
- When all tasks are complete, output exactly: [[ALL DONE]]

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

    # Run Claude Code in headless mode with streaming JSON parsed to human-readable output
    ALL_DONE_FLAG="/tmp/claude_all_done_$$"
    rm -f "$ALL_DONE_FLAG"

    set +e
    claude \
        --dangerously-skip-permissions \
        --print \
        --verbose \
        --output-format stream-json \
        "$CLAUDE_PROMPT" 2>&1 | python3 -u -c "
import sys, json

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        data = json.loads(line)
        msg_type = data.get('type', '')

        if msg_type == 'assistant':
            content = data.get('message', {}).get('content', [])
            for item in content:
                if item.get('type') == 'text':
                    text = item.get('text', '')
                    if text:
                        print(text, flush=True)
                elif item.get('type') == 'tool_use':
                    tool = item.get('name', 'unknown')
                    inp = item.get('input', {})
                    if tool in ('Bash', 'mcp__acp__Bash'):
                        cmd = inp.get('command', '')[:100]
                        print(f'[Tool: {tool}] {cmd}', flush=True)
                    elif tool in ('Read', 'mcp__acp__Read'):
                        print(f'[Tool: {tool}] {inp.get(\"file_path\", \"\")}', flush=True)
                    elif tool in ('Edit', 'mcp__acp__Edit'):
                        print(f'[Tool: {tool}] {inp.get(\"file_path\", \"\")}', flush=True)
                    elif tool in ('Write', 'mcp__acp__Write'):
                        print(f'[Tool: {tool}] {inp.get(\"file_path\", \"\")}', flush=True)
                    elif tool == 'Grep':
                        print(f'[Tool: {tool}] {inp.get(\"pattern\", \"\")}', flush=True)
                    elif tool == 'Glob':
                        print(f'[Tool: {tool}] {inp.get(\"pattern\", \"\")}', flush=True)
                    else:
                        print(f'[Tool: {tool}]', flush=True)
        elif msg_type == 'user':
            content = data.get('message', {}).get('content', [])
            for item in content:
                if item.get('type') == 'tool_result':
                    print('[Tool Result]', flush=True)
        elif msg_type == 'result':
            print('[Session Complete]', flush=True)
        elif msg_type == 'system':
            subtype = data.get('subtype', '')
            if subtype == 'init':
                print(f'[Init] Session: {data.get(\"session_id\", \"\")} CWD: {data.get(\"cwd\", \"\")}', flush=True)
            else:
                print(f'[System] {subtype}', flush=True)

        # Check for ALL DONE
        if '[[ALL DONE]]' in line:
            print('__ALL_DONE_SIGNAL__', flush=True)
    except json.JSONDecodeError:
        # Not JSON, print as-is
        print(line, flush=True)
    except Exception as e:
        pass
" | while IFS= read -r line; do
        echo "$line" | tee -a "$SESSION_LOG"
        if [[ "$line" == "__ALL_DONE_SIGNAL__" ]]; then
            touch "$ALL_DONE_FLAG"
        fi
    done

    result=$?
    set -e

    if [[ $result -ne 0 ]]; then
        echo -e "${YELLOW}Warning: Claude exited with code $result${NC}"
    fi

    # Check for completion signal
    if [[ -f "$ALL_DONE_FLAG" ]] || tail -100 "$SESSION_LOG" | grep -q '\[\[ALL DONE\]\]'; then
        rm -f "$ALL_DONE_FLAG"
        echo -e "${GREEN}[[ALL DONE]] - Implementation complete!${NC}"
        break
    fi
    rm -f "$ALL_DONE_FLAG"

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
