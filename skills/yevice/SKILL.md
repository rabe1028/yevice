---
name: yevice
description: "Estimate AWS / GCP cloud costs from Infrastructure-as-Code (CloudFormation, Terraform, Cloudflare Wrangler) using the yevice Rust CLI. Use whenever the user asks to estimate / forecast / model cloud costs from IaC, generate a cost model, run sensitivity analysis on cost drivers, compare cost between architectures, or optimize cloud spend via MILP — including phrases like \"estimate the monthly cost\", \"how much will this CloudFormation stack cost\", \"compare cost of these two stacks\", \"what if requests 10x\", \"find the cheapest instance mix\", or anything mentioning yevice, cost estimation, IaC pricing, MILP cost optimization, sensitivity analysis on cloud cost, AWS / GCP pricing from templates. Do NOT use for querying past actual billing (AWS Cost Explorer / Billing API), fixing IaC syntax errors, diffing `terraform plan` resources, debugging deploy failures, or generic cloud architecture / performance tuning that does not require a forward-looking cost number."
---

## 1. Scope

yevice is a Rust CLI (`yevice`) that generates a cost model from IaC templates and evaluates monthly cost estimates from that model.
Subcommands: `generate`, `eval`, `compare`, `sensitivity`, `validate`, `simulate`, `optimize`, `update-pricing`, `diagram`.
Supported IaC formats: CloudFormation (YAML/JSON), Terraform (HCL), Cloudflare Wrangler (`wrangler.toml`).
Repository: https://github.com/rabe1028/yevice

---

## 2. Check installation

```bash
which yevice
```

If not found, install from a yevice checkout:

```bash
./scripts/install.sh                 # default ~/.cargo/bin/yevice
./scripts/install.sh --highs         # also enable HiGHS MILP backend
```

Or clone and install:

```bash
git clone https://github.com/rabe1028/yevice.git
cd yevice && ./scripts/install.sh
```

Or directly via cargo:

```bash
cargo install --git https://github.com/rabe1028/yevice.git yevice-cli
```

---

## 3. Typical workflows

### A. Estimate cost from IaC (baseline flow)

```bash
yevice generate --template path/to/template.yaml --name app --output ./out
yevice eval ./out/app.cost_model.json --params ./out/app.usage.yaml
yevice eval ./out/app.cost_model.json --params ./out/app.usage.yaml \
  --display-currency JPY --exchange-rate USD:JPY=155
```

- `--input-format` accepts: `auto` (default), `cfn`, `tf`, `wrangler`
- `--strict` treats parse failures as fatal (recommended in CI)

**Run A.5 before eval** — `usage.yaml` is an empty template after `generate`.

---

### A.5 Interactive parameter completion (Claude-driven)

After `generate`, three files are produced:

- `./out/app.cost_model.json` — expression AST and resource graph
- `./out/app.schema.json` — JSON Schema describing each parameter
- `./out/app.usage.yaml` — parameter file (empty template, must be filled)

**Always inspect `<name>.schema.json` first.** It defines for each parameter:
name, type (`number` / `integer` / `string`), unit (e.g. `requests/month`, `GB`, `ratio`),
description, `default` (if any), and `enum` (if any).

Claude follows this procedure to fill `usage.yaml` interactively:

1. Read `<name>.schema.json` and enumerate all properties.
2. For each property:
   - If `default` is present, show it as the candidate and confirm with the user.
   - If no `default`, ask the user for a value (`AskUserQuestion`), presenting the `unit` and `description` together.
   - For wide-range numeric parameters (e.g. `requests`, `storage_gb`), preview 3–4 log-scale examples: `1e6`, `1e7`, `1e8`, `1e9`.
   - If `enum` is present, list all choices.
3. Write the collected values into `usage.yaml` (Edit or Write tool).
4. Run `yevice eval --params ./out/app.usage.yaml`.

Filled values persist in the file, so subsequent `eval` runs are non-interactive.

> **Important:** When `usage.yaml` is empty or contains placeholder values (e.g. `null`, `0`, `"REPLACE_ME"`), do NOT silently fall back to defaults — ask the user. Cost estimates with wrong assumptions are worse than no estimate.

Unit conventions (schema is authoritative; these are common defaults):

| Unit | Meaning |
|---|---|
| `requests` | per-month request count |
| `storage_gb` | GB-month |
| `egress_gb` | GB out per month |
| `duration_ms` | per-invocation duration |
| `concurrency`, `replicas` | integer count |
| `cache_hit_ratio` | 0.0–1.0 fraction |

---

### B. Sensitivity analysis

Run A.5 first to ensure `usage.yaml` has values before invoking sensitivity.

```bash
yevice sensitivity ./out/app.cost_model.json \
  --params ./out/app.usage.yaml \
  --var requests --min 1e6 --max 1e9 --steps 10
```

---

### C. Cost optimization (MILP)

Enumeration solver (no extra dependency):

```bash
yevice optimize ./out/app.cost_model.json \
  --params ./out/app.usage.yaml \
  --budget 500
```

HiGHS solver (requires `--highs` install):

```bash
yevice optimize ./out/app.cost_model.json \
  --params ./out/app.usage.yaml \
  --budget 500 --solver highs
```

---

### D. Compare scenarios

```bash
yevice compare \
  --baseline ./out/v1.cost_model.json --baseline-params ./out/v1.usage.yaml \
  --candidate ./out/v2.cost_model.json --candidate-params ./out/v2.usage.yaml
```

---

### E. Refresh pricing catalog

```bash
yevice update-pricing --region ap-northeast-1
```

---

## 4. File formats

| Kind | Description |
|---|---|
| Input | CFN YAML/JSON, TF HCL, `wrangler.toml` |
| `<name>.cost_model.json` | Expression AST + resource graph |
| `<name>.schema.json` | JSON Schema for parameter file. **Read this before filling `usage.yaml`** |
| `<name>.usage.yaml` | Parameter file (must be filled before `eval` / `sensitivity`) |
| `pricing-data/<service>.json` | AWS Price List catalog |

---

## 5. Currency and FX

The default catalog currency is USD. To display in another currency:

```bash
--display-currency JPY --exchange-rate USD:JPY=155.0
```

When resources span multiple pricing currencies, the total is labeled `mixed`.
Set `--exchange-rate` to resolve to a single currency.

---

## 6. Parse policy

- **Lenient (default):** best-effort parse; unrecognized resources are skipped with a diagnostic.
- **`--strict`:** any parse failure is fatal; recommended for CI.

Diagnostics have the shape:

```
IacParseDiagnostic { code, severity, source, location, message }
```

---

## 7. Picking the right subcommand

| Question | Subcommand |
|---|---|
| PR cost delta | `compare` |
| What if requests 10x? | `sensitivity` |
| Cheapest SKU mix | `optimize` |
| Capacity / quota check | `validate` |
| Time-varying peak load | `simulate --profile` |
| Just need a number | `generate` then `eval` (run A.5 first) |

---

## 8. Troubleshooting

- **Linker error at build** — install a C linker (`gcc` / `clang`).
- **HiGHS not available** — reinstall with `./scripts/install.sh --highs`.
- **AWS price API rate limit** — retry after a moment; use cached `pricing-data/` if available.
- **TF parse failures** — check the supported Terraform provider/service list in the repo README.
- **Mixed-currency totals** — set `--exchange-rate` to normalise to one currency.
- **`usage.yaml` empty / eval result is $0** — run interactive completion (A.5) to fill in parameter values.

---

## 9. Project home

For installation, design rationale, and release notes, see https://github.com/rabe1028/yevice.
