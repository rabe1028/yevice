# ADR 0003: IaC Parse Failure Policy

**Status:** Proposed

## Context

The three IaC parsers disagree on what "failure" means today:

| Parser | Behaviour on unresolved / missing input |
|---|---|
| **CFN** | Aborts the whole template on a missing parameter (`CfnError::MissingParameters`); same for unresolved `Fn::ImportValue` / mapping lookups. |
| **TF** | Drops unresolved `var.*` / `local.*` / cross-resource refs with `tracing::warn!`; adapters fall back to defaults; an Architecture is still produced. Only HCL syntax errors abort. |
| **Wrangler** | Strict by construction — TOML/JSONC errors abort; no variable layer to be partial about. |

The asymmetry surprises users: a single missing parameter kills CFN
end-to-end, while a similar gap in TF produces a partially populated (and
quietly less accurate) cost model.

## Options

- **A. Strict everywhere** — promote TF warnings to errors. Predictable, but
  regresses today's "TF without tfvars" UX.
- **B. Lenient everywhere** — drop CFN's `MissingParameters` to a warning.
  Quietly wrong numbers are dangerous downstream.
- **C. Two-mode policy with a CLI flag** (recommended).

## Recommendation — Option C, default Lenient

Introduce `ParsePolicy { Lenient, Strict }` threaded through each parser's
public entry point and surfaced as `--strict` / `--lenient`, default
`--lenient`:

- **Lenient:** unresolved refs / missing params emit `tracing::warn!`,
  adapters use fallback defaults, parsing succeeds.
- **Strict:** any unresolved input is a hard error — matches today's CFN
  semantics; lifts TF and Wrangler to the same bar.

A shared `IacParseDiagnostic` list (in `yevice-core::io`) is returned next
to the Architecture so CI can fail on diagnostics even under Lenient.

Lenient by default keeps today's TF UX intact (largest current user base);
`--strict` reinstates CFN's hard-failure for users who relied on it.

## Migration

1. Add `ParsePolicy` + `IacParseDiagnostic` to `yevice-core`.
2. Plumb through `yevice-engine::generate` and the CLI as `--strict`.
3. Convert CFN's hard errors into diagnostics; raise a new
   `CfnError::PolicyViolation` only when `Strict` is requested.

Steps 1–2 are non-breaking; step 3 is gated on a deprecation cycle for the
existing `MissingParameters` variant.

## Out of scope

Generalising `CfnPropertyValue` to an IaC-neutral `IacPropertyValue` is a
separate, larger refactor (it touches all three converters) and is **not**
addressed here. It is noted as a follow-up so this ADR's scope stays
focused on failure policy. Refs #21.
