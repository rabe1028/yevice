# yevice

A CloudFormation cost-function generator. `yevice` parses a CloudFormation
template, derives a parametric cost model from the declared resources, and lets
you evaluate, compare, run sensitivity analyses, validate quota limits, and
simulate load profiles against that model.

## Requirements

- Rust **1.85+** (workspace uses edition 2024). Install via
  [rustup](https://rustup.rs/) — `stable` is sufficient.
- A C linker (`cc` / `clang`) available on `PATH` — required by Cargo for the
  final link step.

CI is pinned to `dtolnay/rust-toolchain@stable`, so any current stable
toolchain works.

## Build

Clone and build the workspace:

```bash
git clone https://github.com/rabe1028/yevice.git
cd yevice

# Debug build (fast compile, slower runtime)
cargo build --workspace

# Release build (recommended for actual use)
cargo build --release --workspace
```

The CLI binary is produced at:

- Debug:   `target/debug/yevice`
- Release: `target/release/yevice`

Run directly without installing:

```bash
cargo run --release -p yevice-cli -- --help
```

## Install

### From a local checkout

`cargo install` builds in release mode and copies the `yevice` binary into
Cargo's bin directory (`~/.cargo/bin` by default, which should already be on
your `PATH` if you installed Rust via rustup).

```bash
# From the repo root
cargo install --path crates/cli/yevice-cli

# Verify
yevice --version
yevice --help
```

To uninstall:

```bash
cargo uninstall yevice-cli
```

### Directly from Git

```bash
cargo install --git https://github.com/rabe1028/yevice.git yevice-cli
```

### Custom install location

```bash
cargo install --path crates/cli/yevice-cli --root /usr/local
# -> /usr/local/bin/yevice
```

## Quick start

The `examples/` directory contains a runnable sample (Lambda + DynamoDB + S3):

```bash
# 1. Generate a cost model from a CloudFormation template
yevice generate \
  --template examples/arch-a.yaml \
  --name arch-a \
  --output arch-a.cost.json

# 2. Evaluate it with usage parameters
yevice eval arch-a.cost.json --params examples/usage.yaml --breakdown

# 3. Compare multiple architectures
yevice generate -t examples/arch-b.yaml -n arch-b -o arch-b.cost.json
yevice compare arch-a.cost.json arch-b.cost.json --params examples/usage.yaml
```

Other available subcommands:

| Command           | Purpose                                             |
| ----------------- | --------------------------------------------------- |
| `generate`        | Build a cost model from a CloudFormation template   |
| `eval`            | Compute monthly cost for given usage parameters     |
| `compare`         | Side-by-side cost comparison of multiple models     |
| `sensitivity`     | Sweep one variable and report cost impact           |
| `validate`        | Check capacity / quota limits against peak usage    |
| `simulate`        | Run a load profile (e.g. hourly pattern) over time  |
| `update-pricing`  | Download fresh AWS pricing data for a region        |

Run `yevice <command> --help` for the full option list.

## Development

Reproduce the CI pipeline locally before pushing:

```bash
scripts/verify-ci.sh              # uses actrun if installed, else cargo
scripts/verify-ci.sh --fallback   # fmt + clippy + test directly
```

See [`scripts/README.md`](scripts/README.md) for details on the `actrun`-based
workflow.

Raw commands (the three checks CI runs in parallel):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --tests
cargo test --workspace
```

## Workspace layout

| Crate                       | Role                                                  |
| --------------------------- | ----------------------------------------------------- |
| `yevice-core`               | Cost-model types, evaluator, parameter resolution     |
| `yevice-cfn`                | CloudFormation template parser                        |
| `yevice-tf`                 | Terraform input support                               |
| `yevice-wrangler`           | Cloudflare Wrangler config support                    |
| `yevice-service-api`        | Service plugin trait / registry                       |
| `yevice-services-aws`       | AWS service cost implementations                      |
| `yevice-services-gcp`       | GCP service cost implementations                      |
| `yevice-pricing`            | Pricing data registry and fetchers                    |
| `yevice-cli`                | `yevice` CLI binary                                   |

## Current limitations

`yevice` is at `v0.1.0` and has the following limitations you should be aware of
before adopting it:

- **Pricing data is mostly `ap-northeast-1` (Tokyo) only.** A subset of services
  (Lambda, EC2, RDS, S3, DynamoDB and a few others) load region-specific data
  via `update-pricing`, but the rest fall back to hard-coded Tokyo prices
  regardless of the `--region` flag. Cost estimates for non-Tokyo regions
  should be treated as approximate.
- **Terraform adapter coverage is a subset of the CloudFormation adapter.**
  ~19 of the ~36 AWS services have a TF adapter; the rest are CFn-only.
- **API stability.** Pre-1.0 — public APIs may break between minor versions.

## License

MIT
