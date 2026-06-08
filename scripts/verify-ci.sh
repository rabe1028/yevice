#!/usr/bin/env bash
# verify-ci.sh — Reproduce the steps from .github/workflows/ci.yml locally
# before pushing. Uses mizchi/actrun if available, otherwise runs the same
# cargo commands directly.
#
# Usage:
#   scripts/verify-ci.sh              # auto-detect (actrun > fallback)
#   scripts/verify-ci.sh --fallback   # skip actrun, run cargo directly
#   scripts/verify-ci.sh --dry-run    # actrun --dry-run (skip cargo fallback)

set -eu

cd "$(dirname "$0")/.."

mode="auto"
dry_run=0
for arg in "$@"; do
    case "$arg" in
        --fallback) mode="fallback" ;;
        --dry-run)  dry_run=1 ;;
        -h|--help)
            sed -n '2,11p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "unknown flag: $arg" >&2
            exit 2
            ;;
    esac
done

run_actrun() {
    local invocation
    if command -v actrun >/dev/null 2>&1; then
        invocation=(actrun)
    elif command -v mise >/dev/null 2>&1 \
        && mise exec -- actrun --help 2>&1 | grep -q "actrun <command>"; then
        invocation=(mise exec -- actrun)
    else
        return 1
    fi
    local args=(workflow run .github/workflows/ci.yml)
    if [ "$dry_run" -eq 1 ]; then
        args+=(--dry-run)
    fi
    echo "[verify-ci] using ${invocation[*]}"
    "${invocation[@]}" "${args[@]}"
}

run_fallback() {
    if [ "$dry_run" -eq 1 ]; then
        echo "[verify-ci] --dry-run requires actrun; skipping fallback" >&2
        exit 2
    fi
    echo "[verify-ci] fallback: cargo fmt --all -- --check"
    cargo fmt --all -- --check
    echo "[verify-ci] fallback: cargo clippy --workspace --all-targets --tests"
    cargo clippy --workspace --all-targets --tests
    echo "[verify-ci] fallback: cargo test --workspace"
    cargo test --workspace
}

case "$mode" in
    fallback)
        run_fallback
        ;;
    auto)
        if run_actrun; then
            exit 0
        else
            echo "[verify-ci] actrun not available; using cargo fallback"
            run_fallback
        fi
        ;;
esac
