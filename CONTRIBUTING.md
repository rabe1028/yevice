# Contributing

Thanks for your interest in `yevice`! Contributions are welcome.

## Development setup

See the [Development](README.md#development) section of the README. The
short version:

```bash
scripts/verify-ci.sh   # mirrors the GitHub Actions pipeline locally
```

## Submitting changes

1. Fork the repo and create a feature branch.
2. Make your changes. Add tests for new behaviour.
3. Run `scripts/verify-ci.sh` and ensure it passes.
4. Open a pull request against `main` describing the change and its motivation.

## Adding a new service

Each cloud service is implemented as a `Service` plugin. To add a new one:

1. Add the cost/capacity model in `crates/services/yevice-services-<provider>/src/services/<name>.rs`.
2. Add the CFn or TF adapter in the sibling `cfn/` or `tf/` directory.
3. Register the service in `crates/services/yevice-services-<provider>/src/lib.rs`.
4. Add fixtures under `crates/cli/yevice-cli/tests/fixtures/` and integration tests.

## Optional: pre-commit hooks

This repo ships a `.pre-commit-config.yaml` compatible with both
[`pre-commit`](https://pre-commit.com/) and the faster Rust-based
[`prek`](https://github.com/j178/prek). After `mise install`, enable hooks
with:

```bash
prek install        # or: pre-commit install
```

This runs `typos`, `taplo fmt --check`, `cargo fmt --check`, and `cargo clippy`
on every commit.

## Reporting issues

For security issues, please see [SECURITY.md](SECURITY.md). For other bugs and
feature requests, open a GitHub issue.
