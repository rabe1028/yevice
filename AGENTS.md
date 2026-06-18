# AGENTS.md — yevice

This file gives AI coding assistants (Claude Code, OpenAI Codex, etc.) project-level context for working in the yevice repository.

## What is yevice

yevice is a Rust CLI that parses Infrastructure-as-Code templates (CloudFormation YAML/JSON, Terraform HCL, Cloudflare Wrangler TOML) and derives a parametric cost model from the declared resources. The model is stored as a JSON expression AST (`cost_model.json`) which can be evaluated, compared between architectures, swept in sensitivity analyses, and optimized via MILP — all without a live AWS/GCP account.

The tool targets the "forward-looking cost estimation" use case: given an IaC template and expected usage parameters, produce a monthly cost estimate in USD (or any other currency via FX conversion). It does not read past billing data or replace AWS Cost Explorer.

## Repository layout

| Crate | Role |
|---|---|
| `yevice-core` | Cost-model types, expression AST, evaluator, parameter resolution, currency phantom typing |
| `yevice-service-api` | Service plugin trait and registry — the contract every service adapter implements |
| `yevice-pricing` | Pricing data registry, catalog loader, and `update-pricing` fetch logic |
| `yevice-cfn` | CloudFormation template parser and IaC adapter |
| `yevice-tf` | Terraform HCL input adapter |
| `yevice-wrangler` | Cloudflare Wrangler (`wrangler.toml`) adapter |
| `yevice-services-aws` | AWS service cost implementations (Lambda, EC2, RDS, S3, DynamoDB, …) |
| `yevice-services-gcp` | GCP service cost implementations |
| `yevice-solver` | MILP optimization backend (enumeration + optional HiGHS) |
| `yevice-output` | Output formatters (table, JSON, diagram) |
| `yevice-engine` | Orchestration layer — wires adapters, services, solver, and output together |
| `yevice-cli` | `yevice` binary — CLI argument parsing via clap |

Crates live under `crates/<group>/<name>` (groups: `core`, `iac`, `services`, `solver`, `output`, `engine`, `cli`).

## Build, test, lint

```bash
cargo build --release          # release build
cargo test --workspace         # full test suite
./scripts/verify-ci.sh         # reproduces the CI pipeline (fmt, clippy, test)
./scripts/install.sh           # install yevice binary into ~/.cargo/bin
./scripts/install.sh --highs   # also enables the HiGHS MILP backend
```

Raw CI checks (run before every PR):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Cost-model workflow (use the Skill)

For end-to-end cost estimation tasks, **use the included Skill** at `skills/yevice/SKILL.md`. It encodes the canonical workflow:

1. `yevice generate --template …` — emits `cost_model.json` + `schema.json` + `usage.yaml`
2. Inspect `schema.json` and **interactively fill `usage.yaml`** (the Skill A.5 procedure)
3. `yevice eval --params usage.yaml` for a single estimate
4. `yevice sensitivity` / `compare` / `optimize` for further analysis

Do NOT bypass the Skill's A.5 step — silently defaulting parameters produces misleading cost numbers.

## Architecture Decision Records

Important design decisions are documented under `docs/adr/`:

- **0001** — Currency and Time Dimensions (phantom currency, FX, mixed-currency rules)
- **0002** — LP / MIP Solver Backend (MilpBackend trait, HiGHS opt-in)
- **0003** — IaC Parse Failure Policy (Lenient / Strict modes, diagnostics shape)
- **0004** — Provider Implementation Pattern (PricingProvider architecture)
- **0005** — IaC Adapter Implementation Pattern (how adapters are structured)

Read the relevant ADR before refactoring its area.

## Code conventions

- Edition 2024, MSRV 1.88+
- Run `cargo fmt --all` before commit
- Run `cargo clippy --workspace --all-targets -- -D warnings` — clippy is set to `pedantic`; suppressions are in `Cargo.toml` `[workspace.lints.clippy]`
- `unsafe_code = "deny"` workspace-wide — do not add unsafe blocks
- Avoid feature flags except for opt-in heavy dependencies (e.g. HiGHS)
- Public API uses `Expr` AST node types and `Currency<T, C>` phantom typing
- Pricing catalogs live in `pricing-data/<service>.json` (gitignored; regenerate with `yevice update-pricing`)

## Pull request hygiene

- Reference the relevant ADR if you change architecture
- Update `CHANGELOG.md` Unreleased section
- For TF / CFN / Wrangler adapter changes, add a smoke-test that exercises the new path
- Keep PRs focused — one concern per PR
