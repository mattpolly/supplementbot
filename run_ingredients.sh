#!/bin/bash
# Run NSAI loop for a batch of ingredients.
#
# Usage:
#   ./run_ingredients.sh "Ingredient 1" "Ingredient 2" ...
#   ./run_ingredients.sh --resume "Ingredient 1" "Ingredient 2" ...
#   ./run_ingredients.sh --retry-skipped
#
# State files (in /tmp/nsai_state/):
#   checkpoint.txt      — last fully completed ingredient
#   skipped.jsonl       — timed-out runs (ingredient, lens, provider, timestamp)
#
# Monitor progress: tail -f /tmp/nsai_batch.log

set -e
cd /srv/www/supplementbot

LENSES=("5th" "10th" "college")
PROVIDERS=("anthropic" "gemini" "grok")
TIMEOUT=180       # seconds per provider run before killing
LOG=/tmp/nsai_batch.log
STATE_DIR=/tmp/nsai_state
CHECKPOINT=$STATE_DIR/checkpoint.txt
SKIPPED=$STATE_DIR/skipped.jsonl

mkdir -p $STATE_DIR

# ── Mode parsing ─────────────────────────────────────────────────────────────

RESUME=false
RETRY_SKIPPED=false

if [ "${1:-}" = "--resume" ]; then
    RESUME=true
    shift
elif [ "${1:-}" = "--retry-skipped" ]; then
    RETRY_SKIPPED=true
    shift
fi

# ── Retry-skipped mode ───────────────────────────────────────────────────────

if [ "$RETRY_SKIPPED" = true ]; then
    if [ ! -f "$SKIPPED" ]; then
        echo "No skipped runs recorded."
        exit 0
    fi
    echo "" | tee -a $LOG
    echo "=== Retry skipped $(date) ===" | tee -a $LOG

    # Read unique (ingredient, lens, provider) triples from skipped.jsonl
    python3 - << 'PYEOF'
import json, subprocess, sys, os, datetime

skipped_file = '/tmp/nsai_state/skipped.jsonl'
log_file = '/tmp/nsai_batch.log'
state_dir = '/tmp/nsai_state'
timeout = 180

entries = []
with open(skipped_file) as f:
    for line in f:
        try:
            entries.append(json.loads(line))
        except:
            pass

if not entries:
    print("No skipped entries to retry.")
    sys.exit(0)

print(f"Retrying {len(entries)} skipped runs...")
remaining = []

for entry in entries:
    ingredient = entry['ingredient']
    lens = entry['lens']
    provider = entry['provider']
    print(f"  {ingredient} [{lens}] → {provider}...", flush=True)

    result = subprocess.run(
        ['timeout', str(timeout), 'cargo', 'run', '--bin', 'supplementbot', '--',
         '-n', ingredient, '--lens', lens, '--provider', provider],
        capture_output=True, text=True, cwd='/srv/www/supplementbot'
    )

    ts = datetime.datetime.now().strftime('%H:%M')
    if result.returncode == 124:
        print(f"    ⚠ Timed out again — keeping in skipped list")
        remaining.append(entry)
    else:
        print(f"    ✓ done ({ts})")
        with open(log_file, 'a') as lf:
            lf.write(f"  Retry ✓ {ingredient} [{lens}] {provider} ({ts})\n")

# Rewrite skipped.jsonl with only the still-failing ones
with open(skipped_file, 'w') as f:
    for entry in remaining:
        f.write(json.dumps(entry) + '\n')

if remaining:
    print(f"\n{len(remaining)} still timing out. Run --retry-skipped again later.")
else:
    print("\nAll skipped runs completed successfully.")
PYEOF
    exit 0
fi

# ── Resume mode: skip already-completed ingredients ──────────────────────────

LAST_DONE=""
if [ "$RESUME" = true ] && [ -f "$CHECKPOINT" ]; then
    LAST_DONE=$(cat "$CHECKPOINT")
    echo "Resuming after: $LAST_DONE" | tee -a $LOG
fi

SKIPPING=true
if [ -z "$LAST_DONE" ]; then
    SKIPPING=false
fi

# ── Main loop ─────────────────────────────────────────────────────────────────

echo "" >> $LOG
echo "=== Batch started $(date) ===" >> $LOG

for ingredient in "$@"; do
    # Resume: skip until we pass the last completed ingredient
    if [ "$SKIPPING" = true ]; then
        if [ "$ingredient" = "$LAST_DONE" ]; then
            SKIPPING=false
        fi
        echo "  (skipping $ingredient — already done)" | tee -a $LOG
        continue
    fi

    echo "" | tee -a $LOG
    echo "════════════════════════════════════════" | tee -a $LOG
    echo "  $ingredient  ($(date '+%H:%M'))" | tee -a $LOG
    echo "════════════════════════════════════════" | tee -a $LOG

    for lens in "${LENSES[@]}"; do
        echo "  [$lens]" | tee -a $LOG
        for provider in "${PROVIDERS[@]}"; do
            echo "    → $provider..." | tee -a $LOG

            set +e
            output=$(timeout $TIMEOUT cargo run --bin supplementbot -- \
                -n "$ingredient" --lens "$lens" --provider "$provider" 2>&1 \
                | grep -E "Also known|KnowledgeGraph \(|complete|Error" || true)
            exit_code=$?
            set -e

            if [ $exit_code -eq 124 ]; then
                msg="       ⚠ TIMED OUT after ${TIMEOUT}s — skipping $provider"
                echo "$msg" | tee -a $LOG
                # Record for later retry
                echo "{\"ingredient\":\"$ingredient\",\"lens\":\"$lens\",\"provider\":\"$provider\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}" >> $SKIPPED
            elif [ $exit_code -ne 0 ] && [ $exit_code -ne 1 ]; then
                echo "       ✗ Error (exit $exit_code) — skipping $provider" | tee -a $LOG
                echo "{\"ingredient\":\"$ingredient\",\"lens\":\"$lens\",\"provider\":\"$provider\",\"timestamp\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\",\"reason\":\"exit_$exit_code\"}" >> $SKIPPED
            else
                echo "$output" | sed 's/^/       /' | tee -a $LOG
            fi
        done
    done

    echo "  ✓ done ($(date '+%H:%M'))" | tee -a $LOG
    # Checkpoint: record this ingredient as fully complete
    echo "$ingredient" > $CHECKPOINT

done

echo "" | tee -a $LOG
echo "=== Batch complete $(date) ===" | tee -a $LOG

# Summary of skipped runs
if [ -f "$SKIPPED" ] && [ -s "$SKIPPED" ]; then
    count=$(wc -l < "$SKIPPED")
    echo "⚠ $count provider run(s) timed out. Retry with: ./run_ingredients.sh --retry-skipped" | tee -a $LOG
fi
