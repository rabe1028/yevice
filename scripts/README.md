# scripts/

## verify-ci.sh

Reproduce the GitHub Actions CI workflow (`.github/workflows/ci.yml`) locally
before pushing.

```bash
scripts/verify-ci.sh              # auto: use actrun if installed, otherwise cargo
scripts/verify-ci.sh --fallback   # skip actrun, just run cargo fmt/clippy/test
scripts/verify-ci.sh --dry-run    # actrun --dry-run (plan only, requires actrun)
```

### Installing `actrun` (mizchi/actrun)

[mizchi/actrun](https://github.com/mizchi/actrun) is a local GitHub Actions
runner with a `gh`-compatible CLI.

**Recommended: via [mise](https://mise.jdx.dev/)** — the repo's `mise.toml`
pins it to a known-working version (0.17.0 — later 0.18+ npm builds ship a
JS syntax error that aborts on startup).

```bash
mise install                       # one-time, reads mise.toml
mise exec -- actrun --version      # ad-hoc invocation
# or with mise's shell hook active, just `actrun ...` works
```

Other install methods (in case mise isn't preferred):

```bash
# Official release tarball
curl -fsSL https://raw.githubusercontent.com/mizchi/actrun/main/install.sh | sh

# Docker
alias actrun='docker run --rm -v "$PWD":/workspace -w /workspace ghcr.io/mizchi/actrun'

# Nix
nix profile install github:mizchi/actrun
```

`actrun.toml` at the repo root configures actrun to:
- Skip `actions/checkout` and `Swatinem/rust-cache` locally
- Override `dtolnay/rust-toolchain` to just print the local `rustc --version`
  (the local toolchain is used as-is)
- Trust all third-party actions without prompting

### Fallback behaviour

If `actrun` is not on `PATH`, `verify-ci.sh` runs the same three commands
directly:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --tests`
3. `cargo test --workspace`

This mirrors the three parallel jobs in `.github/workflows/ci.yml`.
