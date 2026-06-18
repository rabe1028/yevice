#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<EOF
Usage: scripts/install.sh [OPTIONS]

OPTIONS:
  --highs              Enable HiGHS MILP backend (--features highs)
  --root PATH          Install root (default: ~/.cargo). Binary is PATH/bin/yevice
  --force              Force overwrite even if already installed
  --locked             Use Cargo.lock strictly (default: true)
  --no-locked          Allow Cargo.lock to be updated
  --dry-run            Print the command without executing
  -h, --help           Show this help

EOF
}

HIGHS=false
ROOT=""
FORCE=false
LOCKED=true
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --highs)
      HIGHS=true
      shift
      ;;
    --root)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "Error: --root requires a PATH argument" >&2
        exit 2
      fi
      ROOT="$2"
      shift 2
      ;;
    --force)
      FORCE=true
      shift
      ;;
    --locked)
      LOCKED=true
      shift
      ;;
    --no-locked)
      LOCKED=false
      shift
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Error: Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

cd "$(dirname "$0")/.."

if ! command -v cargo &>/dev/null; then
  echo "Error: Rust toolchain not found. Install via https://rustup.rs/" >&2
  exit 1
fi

CMD=(cargo install --path crates/cli/yevice-cli)

if [[ "$LOCKED" == true ]]; then
  CMD+=(--locked)
fi

if [[ "$FORCE" == true ]]; then
  CMD+=(--force)
fi

if [[ -n "$ROOT" ]]; then
  CMD+=(--root "$ROOT")
fi

if [[ "$HIGHS" == true ]]; then
  CMD+=(--features highs)
fi

if [[ "$DRY_RUN" == true ]]; then
  echo "${CMD[*]}"
  exit 0
fi

"${CMD[@]}"

if [[ -n "$ROOT" ]]; then
  echo "Installed: ${ROOT}/bin/yevice"
else
  echo "Installed: ${CARGO_HOME:-$HOME/.cargo}/bin/yevice"
fi
